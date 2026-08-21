//! Instruction dispatch.
//!
//! One `match` over [`Op`] covering the IL subset RustCLR executes. Numeric
//! behaviour follows ECMA-335 III.1.5 (binary numeric promotion) and III.3
//! (per-instruction semantics); where this runtime deliberately narrows the
//! spec, the arm says so.

use super::*;
use crate::value::RawPtr;
use rustclr_metadata::TableId;

#[allow(unused_imports)]
use crate::prelude::*;

impl Interpreter {
    pub(super) fn execute(&mut self, ins: &Instruction) -> ExecResult<StepOutcome> {
        // The `constrained.` prefix applies to the next call only.
        let constrained = if ins.op == Op::Constrained {
            None
        } else {
            self.frame().constrained.take()
        };

        match ins.op {
            // -- no-ops and prefixes -----------------------------------------
            Op::Nop | Op::Break | Op::Volatile | Op::Readonly | Op::Tail | Op::Unaligned
            | Op::No => {}

            Op::Constrained => {
                self.frame().constrained = ins.operand.as_token();
            }

            // -- constants ----------------------------------------------------
            Op::Ldnull => self.push(Value::Null),
            Op::LdcI4M1 => self.push(Value::I32(-1)),
            Op::LdcI40 => self.push(Value::I32(0)),
            Op::LdcI41 => self.push(Value::I32(1)),
            Op::LdcI42 => self.push(Value::I32(2)),
            Op::LdcI43 => self.push(Value::I32(3)),
            Op::LdcI44 => self.push(Value::I32(4)),
            Op::LdcI45 => self.push(Value::I32(5)),
            Op::LdcI46 => self.push(Value::I32(6)),
            Op::LdcI47 => self.push(Value::I32(7)),
            Op::LdcI48 => self.push(Value::I32(8)),
            Op::LdcI4S | Op::LdcI4 => {
                let v = ins.operand.as_i32().unwrap_or(0);
                self.push(Value::I32(v));
            }
            Op::LdcI8 => {
                let Operand::I64(v) = ins.operand else {
                    return Err(self.bad_operand(ins));
                };
                self.push(Value::I64(v));
            }
            Op::LdcR4 | Op::LdcR8 => {
                let Operand::F64(v) = ins.operand else {
                    return Err(self.bad_operand(ins));
                };
                self.push(Value::F(v));
            }

            Op::Ldstr => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let handle = self.load_literal(token)?;
                self.push(Value::Obj(handle));
            }

            // -- stack manipulation -------------------------------------------
            Op::Dup => {
                let v = self.pop()?;
                self.push(v.clone());
                self.push(v);
            }
            Op::Pop => {
                self.pop()?;
            }

            // -- arguments and locals ------------------------------------------
            Op::Ldarg0 => self.load_arg(0)?,
            Op::Ldarg1 => self.load_arg(1)?,
            Op::Ldarg2 => self.load_arg(2)?,
            Op::Ldarg3 => self.load_arg(3)?,
            Op::LdargS | Op::Ldarg => {
                let i = ins.operand.as_var().ok_or_else(|| self.bad_operand(ins))?;
                self.load_arg(i as usize)?;
            }
            Op::StargS | Op::Starg => {
                let i = ins.operand.as_var().ok_or_else(|| self.bad_operand(ins))? as usize;
                let v = self.pop()?;
                let f = self.frame();
                if i < f.args.len() {
                    f.args[i] = v;
                }
            }
            Op::LdargaS | Op::Ldarga => {
                let i = ins.operand.as_var().ok_or_else(|| self.bad_operand(ins))?;
                let frame = self.frame_ref().id;
                self.push(Value::Ref(ByRef::Arg { frame, index: i }));
            }

            Op::Ldloc0 => self.load_local(0)?,
            Op::Ldloc1 => self.load_local(1)?,
            Op::Ldloc2 => self.load_local(2)?,
            Op::Ldloc3 => self.load_local(3)?,
            Op::LdlocS | Op::Ldloc => {
                let i = ins.operand.as_var().ok_or_else(|| self.bad_operand(ins))?;
                self.load_local(i as usize)?;
            }
            Op::Stloc0 => self.store_local(0)?,
            Op::Stloc1 => self.store_local(1)?,
            Op::Stloc2 => self.store_local(2)?,
            Op::Stloc3 => self.store_local(3)?,
            Op::StlocS | Op::Stloc => {
                let i = ins.operand.as_var().ok_or_else(|| self.bad_operand(ins))?;
                self.store_local(i as usize)?;
            }
            Op::LdlocaS | Op::Ldloca => {
                let i = ins.operand.as_var().ok_or_else(|| self.bad_operand(ins))?;
                let frame = self.frame_ref().id;
                self.push(Value::Ref(ByRef::Local { frame, index: i }));
            }

            // -- arithmetic -----------------------------------------------------
            Op::Add => self.binary_numeric(ins.op, false)?,
            Op::Sub => self.binary_numeric(ins.op, false)?,
            Op::Mul => self.binary_numeric(ins.op, false)?,
            Op::Div => self.binary_numeric(ins.op, false)?,
            Op::Rem => self.binary_numeric(ins.op, false)?,
            Op::DivUn | Op::RemUn => self.binary_numeric(ins.op, true)?,
            Op::And | Op::Or | Op::Xor => self.binary_integer(ins.op)?,
            Op::Shl | Op::Shr | Op::ShrUn => self.shift(ins.op)?,
            Op::AddOvf | Op::SubOvf | Op::MulOvf => self.binary_checked(ins.op, false)?,
            Op::AddOvfUn | Op::SubOvfUn | Op::MulOvfUn => self.binary_checked(ins.op, true)?,

            Op::Neg => {
                let v = self.pop()?;
                let out = match v {
                    Value::I32(a) => Value::I32(a.wrapping_neg()),
                    Value::I64(a) => Value::I64(a.wrapping_neg()),
                    Value::NativeInt(a) => Value::NativeInt(a.wrapping_neg()),
                    Value::F(a) => Value::F(-a),
                    other => return Err(self.type_error("neg", &other)),
                };
                self.push(out);
            }
            Op::Not => {
                let v = self.pop()?;
                let out = match v {
                    Value::I32(a) => Value::I32(!a),
                    Value::I64(a) => Value::I64(!a),
                    Value::NativeInt(a) => Value::NativeInt(!a),
                    other => return Err(self.type_error("not", &other)),
                };
                self.push(out);
            }

            Op::Ckfinite => {
                let v = self.pop()?;
                match v {
                    Value::F(f) if f.is_finite() => self.push(Value::F(f)),
                    Value::F(_) => {
                        return Err(ExecutionError::exception(
                            ClrExceptionKind::Arithmetic,
                            "Value is not a finite number.",
                        ))
                    }
                    other => return Err(self.type_error("ckfinite", &other)),
                }
            }

            // -- conversions -----------------------------------------------------
            Op::ConvI1 | Op::ConvI2 | Op::ConvI4 | Op::ConvI8 | Op::ConvU1 | Op::ConvU2
            | Op::ConvU4 | Op::ConvU8 | Op::ConvI | Op::ConvU | Op::ConvR4 | Op::ConvR8
            | Op::ConvRUn => {
                let v = self.pop()?;
                let out = self.convert(ins.op, &v)?;
                self.push(out);
            }
            Op::ConvOvfI1 | Op::ConvOvfI2 | Op::ConvOvfI4 | Op::ConvOvfI8 | Op::ConvOvfU1
            | Op::ConvOvfU2 | Op::ConvOvfU4 | Op::ConvOvfU8 | Op::ConvOvfI | Op::ConvOvfU
            | Op::ConvOvfI1Un | Op::ConvOvfI2Un | Op::ConvOvfI4Un | Op::ConvOvfI8Un
            | Op::ConvOvfU1Un | Op::ConvOvfU2Un | Op::ConvOvfU4Un | Op::ConvOvfU8Un
            | Op::ConvOvfIUn | Op::ConvOvfUUn => {
                let v = self.pop()?;
                let out = self.convert_checked(ins.op, &v)?;
                self.push(out);
            }

            // -- comparison ------------------------------------------------------
            Op::Ceq => {
                let (a, b) = self.pop2()?;
                let r = self.compare_equal(&a, &b);
                self.push(Value::I32(r as i32));
            }
            Op::Cgt => {
                let (a, b) = self.pop2()?;
                let r = self.compare_ordered(&a, &b, false)? == Some(core::cmp::Ordering::Greater);
                self.push(Value::I32(r as i32));
            }
            Op::CgtUn => {
                let (a, b) = self.pop2()?;
                // `cgt.un` on floats is true when unordered, which is how
                // `!(a <= b)` is compiled.
                let r = match self.compare_ordered(&a, &b, true)? {
                    Some(core::cmp::Ordering::Greater) => true,
                    None => true,
                    _ => false,
                };
                self.push(Value::I32(r as i32));
            }
            Op::Clt => {
                let (a, b) = self.pop2()?;
                let r = self.compare_ordered(&a, &b, false)? == Some(core::cmp::Ordering::Less);
                self.push(Value::I32(r as i32));
            }
            Op::CltUn => {
                let (a, b) = self.pop2()?;
                let r = match self.compare_ordered(&a, &b, true)? {
                    Some(core::cmp::Ordering::Less) => true,
                    None => true,
                    _ => false,
                };
                self.push(Value::I32(r as i32));
            }

            // -- branches ---------------------------------------------------------
            Op::Br | Op::BrS => {
                let t = ins.operand.as_target().ok_or_else(|| self.bad_operand(ins))?;
                self.branch_to(t)?;
            }
            Op::Brtrue | Op::BrtrueS => {
                let v = self.pop()?;
                if v.is_truthy() {
                    let t = ins.operand.as_target().ok_or_else(|| self.bad_operand(ins))?;
                    self.branch_to(t)?;
                }
            }
            Op::Brfalse | Op::BrfalseS => {
                let v = self.pop()?;
                if !v.is_truthy() {
                    let t = ins.operand.as_target().ok_or_else(|| self.bad_operand(ins))?;
                    self.branch_to(t)?;
                }
            }
            Op::Beq | Op::BeqS | Op::BneUn | Op::BneUnS | Op::Bge | Op::BgeS | Op::Bgt
            | Op::BgtS | Op::Ble | Op::BleS | Op::Blt | Op::BltS | Op::BgeUn | Op::BgeUnS
            | Op::BgtUn | Op::BgtUnS | Op::BleUn | Op::BleUnS | Op::BltUn | Op::BltUnS => {
                let (a, b) = self.pop2()?;
                if self.conditional_branch_taken(ins.op, &a, &b)? {
                    let t = ins.operand.as_target().ok_or_else(|| self.bad_operand(ins))?;
                    self.branch_to(t)?;
                }
            }
            Op::Switch => {
                let Operand::Targets(targets) = &ins.operand else {
                    return Err(self.bad_operand(ins));
                };
                let index = self.pop()?.as_i32().unwrap_or(-1);
                if index >= 0 && (index as usize) < targets.len() {
                    let t = targets[index as usize];
                    self.branch_to(t)?;
                }
                // Out of range falls through, as the spec requires.
            }

            // -- calls -------------------------------------------------------------
            Op::Call => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                return self.do_call(token, false, None);
            }
            Op::Callvirt => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                return self.do_call(token, true, constrained);
            }
            Op::Calli => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                return self.do_calli(token);
            }
            Op::Newobj => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                return self.do_newobj(token);
            }
            Op::Ret => {
                // Collect *before* popping: once the return value is in a local
                // Rust variable nothing roots it, and a collection here would
                // reclaim the object about to be handed to the caller.
                self.maybe_collect();

                let method = self.frame_ref().method;
                let returns_void = self.loader.registry.method(method).returns_void();
                let value = if returns_void { None } else { Some(self.pop()?) };
                return self.do_return(value);
            }
            Op::Ldftn => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let m = self.resolve_method(token)?;
                self.push(Value::FnPtr(m));
            }
            Op::Ldvirtftn => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let obj = self.pop()?;
                let declared = self.resolve_method(token)?;
                let target = match obj.as_handle() {
                    Some(h) if !h.is_null() => self.resolve_virtual_target(h, declared)?,
                    _ => declared,
                };
                self.push(Value::FnPtr(target));
            }

            // -- fields --------------------------------------------------------------
            Op::Ldfld => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let obj = self.pop()?;
                let v = self.load_field(&obj, token)?;
                self.push(v);
            }
            Op::Stfld => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let value = self.pop()?;
                let obj = self.pop()?;
                self.store_field(&obj, token, value)?;
            }
            Op::Ldflda => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let obj = self.pop()?;
                let field = self.resolve_field(token)?;

                // The receiver is either a heap object, or a managed pointer to
                // a value type — `p.X` where `p` is a struct local compiles to
                // `ldloca p; ldflda X`, and there is no object to point at.
                let address = match &obj {
                    Value::Ref(base) => {
                        let container = self.load_indirect(base.clone())?;
                        match container {
                            Value::Struct(s) => {
                                let slot = self
                                    .field_slot(s.type_id, field)
                                    .ok_or_else(|| self.missing_field(field))?
                                    as u32;
                                ByRef::StructField { base: Box::new(base.clone()), slot }
                            }
                            // A pointer to a slot holding a reference: the
                            // field lives on the object it refers to.
                            Value::Obj(h) if !h.is_null() => {
                                let type_id = self.type_of(h).unwrap_or(TypeId::INVALID);
                                let slot = self
                                    .field_slot(type_id, field)
                                    .ok_or_else(|| self.missing_field(field))?
                                    as u32;
                                ByRef::Field { object: h, slot }
                            }
                            _ => return Err(ExecutionError::null_reference()),
                        }
                    }
                    _ => {
                        let handle = obj
                            .as_handle()
                            .filter(|h| !h.is_null())
                            .ok_or_else(ExecutionError::null_reference)?;
                        let type_id = self.type_of(handle).unwrap_or(TypeId::INVALID);
                        let slot = self
                            .field_slot(type_id, field)
                            .ok_or_else(|| self.missing_field(field))?
                            as u32;
                        ByRef::Field { object: handle, slot }
                    }
                };
                self.push(Value::Ref(address));
            }
            Op::Ldsfld => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let field = self.resolve_field(token)?;
                self.ensure_cctor(self.loader.registry.field(field).declaring_type)?;
                let v = self.loader.static_value(field).clone();
                self.push(v);
            }
            Op::Stsfld => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let field = self.resolve_field(token)?;
                self.ensure_cctor(self.loader.registry.field(field).declaring_type)?;
                let v = self.pop()?;
                self.loader.set_static(field, v);
            }
            Op::Ldsflda => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let field = self.resolve_field(token)?;
                let type_id = self.loader.registry.field(field).declaring_type;
                self.ensure_cctor(type_id)?;
                self.push(Value::Ref(ByRef::Static { type_id, slot: field.0 }));
            }

            // -- indirect access -------------------------------------------------------
            Op::LdindI1 | Op::LdindU1 | Op::LdindI2 | Op::LdindU2 | Op::LdindI4 | Op::LdindU4
            | Op::LdindI8 | Op::LdindI | Op::LdindR4 | Op::LdindR8 | Op::LdindRef => {
                let addr = self.pop()?;
                // A raw pointer is byte-addressed; a managed one is a path.
                let v = match addr {
                    Value::Ptr(p) => self.load_through_pointer(p, ins.op)?,
                    other => {
                        let r = other.as_byref().ok_or_else(ExecutionError::null_reference)?;
                        self.load_indirect(r)?
                    }
                };
                self.push(self.narrow_for_ldind(ins.op, v));
            }
            Op::StindI1 | Op::StindI2 | Op::StindI4 | Op::StindI8 | Op::StindI | Op::StindR4
            | Op::StindR8 | Op::StindRef => {
                let value = self.pop()?;
                let addr = self.pop()?;
                let narrowed = self.narrow_for_stind(ins.op, value);
                match addr {
                    Value::Ptr(p) => self.store_through_pointer(p, narrowed, ins.op)?,
                    other => {
                        let r = other.as_byref().ok_or_else(ExecutionError::null_reference)?;
                        self.store_indirect(r, narrowed)?;
                    }
                }
            }

            // -- objects ------------------------------------------------------------------
            Op::Initobj => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let addr = self.pop()?;
                let r = addr.as_byref().ok_or_else(ExecutionError::null_reference)?;
                let type_id = self.resolve_type(token)?;
                let zero = self.zero_of(type_id);
                self.store_indirect(r, zero)?;
            }
            Op::Ldobj => {
                let addr = self.pop()?;
                let r = addr.as_byref().ok_or_else(ExecutionError::null_reference)?;
                let v = self.load_indirect(r)?;
                self.push(v);
            }
            Op::Stobj => {
                let value = self.pop()?;
                let addr = self.pop()?;
                let r = addr.as_byref().ok_or_else(ExecutionError::null_reference)?;
                self.store_indirect(r, value)?;
            }
            Op::Cpobj => {
                let src = self.pop()?;
                let dst = self.pop()?;
                let sr = src.as_byref().ok_or_else(ExecutionError::null_reference)?;
                let dr = dst.as_byref().ok_or_else(ExecutionError::null_reference)?;
                let v = self.load_indirect(sr)?;
                self.store_indirect(dr, v)?;
            }

            Op::Box => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let v = self.pop()?;
                let type_id = self.resolve_type(token)?;
                // Boxing a reference type is a no-op per III.4.1.
                if self.loader.registry.ty(type_id).kind.is_value_like() {
                    let h = self.box_value(type_id, v);
                    self.push(Value::Obj(h));
                } else {
                    self.push(v);
                }
            }
            Op::Unbox | Op::UnboxAny => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let v = self.pop()?;
                let type_id = self.resolve_type(token)?;
                let out = self.unbox(v, type_id, ins.op == Op::UnboxAny)?;
                self.push(out);
            }
            Op::Castclass => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let v = self.pop()?;
                let target = self.resolve_type(token)?;
                match v.as_handle() {
                    Some(h) if h.is_null() => self.push(Value::Null),
                    Some(h) => {
                        let actual = self.type_of(h).unwrap_or(TypeId::INVALID);
                        if self.loader.registry.is_assignable_to(actual, target) {
                            self.push(Value::Obj(h));
                        } else {
                            let from = self.type_name_of(h);
                            let to = self.loader.registry.ty(target).full_name();
                            return Err(ExecutionError::invalid_cast(&from, &to));
                        }
                    }
                    None => self.push(v),
                }
            }
            Op::Isinst => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let v = self.pop()?;
                let target = self.resolve_type(token)?;
                match v.as_handle() {
                    Some(h) if !h.is_null() => {
                        let actual = self.type_of(h).unwrap_or(TypeId::INVALID);
                        if self.loader.registry.is_assignable_to(actual, target) {
                            self.push(Value::Obj(h));
                        } else {
                            self.push(Value::Null);
                        }
                    }
                    _ => self.push(Value::Null),
                }
            }

            // -- arrays ----------------------------------------------------------------------
            Op::Newarr => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let count = self.pop()?.as_i64().unwrap_or(-1);
                if count < 0 {
                    return Err(ExecutionError::exception(
                        ClrExceptionKind::ArgumentOutOfRange,
                        "Non-negative number required.",
                    ));
                }
                let element = self.resolve_type(token)?;
                let h = self.alloc_array(element, count as usize);
                self.push(Value::Obj(h));
                self.maybe_collect();
            }
            Op::Ldlen => {
                let v = self.pop()?;
                let h = v.as_handle().filter(|h| !h.is_null()).ok_or_else(
                    ExecutionError::null_reference,
                )?;
                let len = self
                    .heap.with::<ClrArray, _>(h, |a| a.len())
                    .ok_or_else(ExecutionError::null_reference)?;
                self.push(Value::NativeInt(len as i64));
            }
            Op::LdelemI1 | Op::LdelemU1 | Op::LdelemI2 | Op::LdelemU2 | Op::LdelemI4
            | Op::LdelemU4 | Op::LdelemI8 | Op::LdelemI | Op::LdelemR4 | Op::LdelemR8
            | Op::LdelemRef | Op::Ldelem => {
                let (array, index) = self.pop2()?;
                let v = self.load_element(&array, &index)?;
                self.push(v);
            }
            Op::StelemI1 | Op::StelemI2 | Op::StelemI4 | Op::StelemI8 | Op::StelemI
            | Op::StelemR4 | Op::StelemR8 | Op::StelemRef | Op::Stelem => {
                let value = self.pop()?;
                let index = self.pop()?;
                let array = self.pop()?;
                self.store_element(&array, &index, value)?;
            }
            Op::Ldelema => {
                let (array, index) = self.pop2()?;
                let h = array.as_handle().filter(|h| !h.is_null()).ok_or_else(
                    ExecutionError::null_reference,
                )?;
                let i = index.as_i64().unwrap_or(-1);
                let len = self.heap.with::<ClrArray, _>(h, |a| a.len()).unwrap_or(0);
                if i < 0 || i as usize >= len {
                    return Err(ExecutionError::index_out_of_range(i, len));
                }
                self.push(Value::Ref(ByRef::ArrayElement { array: h, index: i as u32 }));
            }

            // -- exceptions --------------------------------------------------------------------
            Op::Throw => {
                let v = self.pop()?;
                let h = v.as_handle().filter(|h| !h.is_null()).ok_or_else(
                    ExecutionError::null_reference,
                )?;
                return Err(self.exception_from_handle(h));
            }
            Op::Rethrow => {
                let pending = self.frame().in_flight.take();
                return Err(match pending {
                    Some(e) => *e,
                    None => ExecutionError::exception(
                        ClrExceptionKind::InvalidOperation,
                        "rethrow outside of a catch handler",
                    ),
                });
            }
            Op::Leave | Op::LeaveS => {
                let target = ins.operand.as_target().ok_or_else(|| self.bad_operand(ins))?;
                return self.do_leave(target);
            }
            Op::Endfinally => {
                return self.do_endfinally();
            }
            Op::Endfilter => {
                // Ends the filter and hands its verdict back to the unwind.
                //
                // `do_return` is what carries it: the filter frame sits at the
                // frame floor `run_filter` established, so returning a value
                // from it goes to that caller rather than onto the evaluation
                // stack of the frame being unwound.
                let verdict = self.pop()?;
                if self.frame_ref().is_filter {
                    // The filter ran over a copy of the unwinding frame's
                    // locals; write them back so anything it changed survives.
                    //
                    // `catch (E e) when (Log(ref buffer))` is the case that
                    // needs this: the `ref` points into the filter frame's
                    // copy, and without the write-back the append vanishes and
                    // the filter looks as though it never ran. The frame
                    // underneath is the one being unwound — `run_filter` pushes
                    // this frame directly onto it.
                    let locals = self.frame_ref().locals.clone();
                    let depth = self.frames.len();
                    if depth >= 2 {
                        self.frames[depth - 2].locals = locals;
                    }
                    return self.do_return(Some(verdict));
                }
                // Reached outside a filter frame, which means the IL fell into
                // a filter block during ordinary flow. That is malformed, and
                // dropping the value is what the previous behaviour did.
            }

            // -- metadata ------------------------------------------------------------------------
            Op::Ldtoken => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                // A *type* handle is resolved here rather than carried as a raw
                // token, because a token only means anything relative to the
                // assembly that emitted it — and the handle may be stored in a
                // local and consumed somewhere else entirely. `typeof(T)`
                // becomes `ldtoken T; call Type::GetTypeFromHandle`, and after
                // this the second call is the identity function.
                //
                // Field and method handles stay raw: `InitializeArray` wants
                // the token, and resolves it against the current assembly at
                // the point of use.
                let is_type_token = matches!(
                    token.table(),
                    Some(TableId::TypeDef) | Some(TableId::TypeRef) | Some(TableId::TypeSpec)
                );
                if is_type_token {
                    // `typeof(T)` on a generic parameter has no answer here:
                    // the argument was erased, and `resolve_type` would report
                    // `System.Object`. That is a plausible-looking wrong answer
                    // — the worst kind — so it is refused instead.
                    // A type parameter is answerable when the frame can name
                    // it — a method one from the instantiation, a class one
                    // from the receiver. `resolve_type` handles both above.
                    // What is left is a class parameter in a *static* method,
                    // where there is no receiver to ask.
                    if self.frame_generic_argument(token).is_none()
                        && self.token_names_a_generic_parameter(token)
                    {
                        return Err(ExecutionError::exception(
                            ClrExceptionKind::NotSupported,
                            "typeof(T) cannot name a generic parameter on this runtime: type arguments are erased. See docs/limitations.md.",
                        ));
                    }
                    if let Ok(type_id) = self.resolve_type(token) {
                        let handle = self.type_object(type_id);
                        self.push(Value::Obj(handle));
                        return Ok(StepOutcome::Continue);
                    }
                }
                self.push(Value::I32(token.raw() as i32));
            }
            Op::Sizeof => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let type_id = self.resolve_type(token)?;
                self.push(Value::I32(self.size_of(type_id) as i32));
            }

            // -- raw memory ---------------------------------------------------------------
            //
            // `stackalloc` asks for a byte range. It gets one on the managed
            // heap rather than the native stack: the pointer that comes back
            // roots it, so it lives exactly as long as something can reach it,
            // which is a stronger guarantee than the stack frame it would have
            // had. Nothing observable depends on where it lives.
            Op::Localloc => {
                let size = self.pop()?.as_i64().unwrap_or(0).max(0) as usize;
                let buffer = self.alloc_byte_buffer(size);
                self.push(Value::Ptr(RawPtr { buffer, offset: 0 }));
            }
            Op::Cpblk => {
                let count = self.pop()?.as_i64().unwrap_or(0).max(0) as usize;
                let from = self.pop()?;
                let to = self.pop()?;
                self.copy_block(to, from, count)?;
            }
            Op::Initblk => {
                let count = self.pop()?.as_i64().unwrap_or(0).max(0) as usize;
                let fill = self.pop()?.as_i32().unwrap_or(0) as u8;
                let to = self.pop()?;
                self.fill_block(to, fill, count)?;
            }

            // -- deliberately unsupported ---------------------------------------------------------
            Op::Arglist
            | Op::Mkrefany
            | Op::Refanyval
            | Op::Refanytype
            | Op::Jmp => {
                return Err(ExecutionError::Unsupported(format!(
                    "`{}` is not implemented by this runtime",
                    ins.op.name()
                )));
            }
        }

        Ok(StepOutcome::Continue)
    }

    // -- diagnostics helpers -------------------------------------------------

    fn bad_operand(&self, ins: &Instruction) -> ExecutionError {
        ExecutionError::InvalidProgram(format!(
            "`{}` at IL_{:04X} has a malformed operand",
            ins.op.name(),
            ins.offset
        ))
    }

    fn type_error(&self, op: &str, v: &Value) -> ExecutionError {
        ExecutionError::InvalidProgram(format!("`{op}` cannot operate on {}", v.kind_name()))
    }

    fn missing_field(&self, field: FieldId) -> ExecutionError {
        ExecutionError::exception(
            ClrExceptionKind::MissingField,
            format!("Field '{}' was not found on the instance.", self.loader.registry.field(field).name),
        )
    }

    // -- locals and arguments -------------------------------------------------

    fn load_arg(&mut self, index: usize) -> ExecResult<()> {
        let v = self
            .frame_ref()
            .args
            .get(index)
            .cloned()
            .ok_or_else(|| ExecutionError::InvalidProgram(format!("argument {index} is out of range")))?;
        self.push(v);
        Ok(())
    }

    fn load_local(&mut self, index: usize) -> ExecResult<()> {
        let v = self
            .frame_ref()
            .locals
            .get(index)
            .cloned()
            .ok_or_else(|| ExecutionError::InvalidProgram(format!("local {index} is out of range")))?;
        self.push(v);
        Ok(())
    }

    fn store_local(&mut self, index: usize) -> ExecResult<()> {
        let v = self.pop()?;
        let f = self.frame();
        if index >= f.locals.len() {
            f.locals.resize(index + 1, Value::Null);
        }
        f.locals[index] = v;
        Ok(())
    }

    // -- literals ---------------------------------------------------------------

    fn load_literal(&mut self, token: Token) -> ExecResult<Handle> {
        let assembly = self.frame_ref().assembly;
        let key = (assembly.0, token.row());
        if let Some(h) = self.literal_cache.get(&key) {
            if self.heap.is_valid(*h) {
                return Ok(*h);
            }
        }
        let heap_bytes = &self.loader.assembly(assembly).user_strings;
        let text = rustclr_metadata::heaps::UserStringHeap(heap_bytes)
            .get(token.row())
            .map_err(ExecutionError::Metadata)?;
        let handle = self.alloc_string(&text);
        self.literal_cache.insert(key, handle);
        Ok(handle)
    }

    // -- token resolution ---------------------------------------------------------

    pub(super) fn resolve_type(&mut self, token: Token) -> ExecResult<TypeId> {
        // A `!!N` in the executing method's own body names one of *its* type
        // arguments, and the instantiation being run knows what those are even
        // though the shared body does not. Consulting it here is what makes
        // `typeof(T)`, `default(T)` and `x is T` work inside a generic method.
        if let Some(resolved) = self.frame_generic_argument(token) {
            return Ok(resolved);
        }
        let assembly = self.frame_ref().assembly;
        self.loader
            .resolve_type_token(self.loader.assembly(assembly), token)
            .ok_or(ExecutionError::UnresolvedToken { token, context: "type".into() })
    }

    /// Type arguments the call site's `TypeSpec` names, for a framework generic.
    fn call_site_type_arguments(&self, token: Token) -> Vec<TypeId> {
        let Some(assembly) = self.current_assembly() else { return Vec::new() };
        self.loader
            .member_ref_type_args(assembly, token)
            .map(|a| a.to_vec())
            .unwrap_or_default()
    }

    /// The closed construction a member reference names, if it names one.
    fn constructed_owner(&self, token: Token) -> Option<TypeId> {
        if token.table() != Some(TableId::MemberRef) {
            return None;
        }
        let assembly = self.frame_ref().assembly;
        self.loader
            .assembly(assembly)
            .member_ref_owner
            .get(&token.row())
            .copied()
    }

    /// The type a `!!N` or `!N` token stands for in the frame currently
    /// executing.
    ///
    /// Two sources, and they are different in kind:
    ///
    /// * `!!N` — a **method** type parameter. The call site's `MethodSpec`
    ///   carried the argument and the instantiation recorded it, so the
    ///   executing method knows it directly.
    /// * `!N` — a **class** type parameter. The body is shared by every
    ///   construction, so the method cannot know; the *receiver* can. `this`
    ///   is an instance of `Box<int>` or of `Box<string>`, and those are
    ///   different runtime types carrying different arguments.
    ///
    /// `None` when neither source has an answer — a `!N` in a static method on
    /// a generic type, where there is no receiver to ask, and a `!!N` reached
    /// without a `MethodSpec`. Both still refuse rather than guessing.
    fn frame_generic_argument(&self, token: Token) -> Option<TypeId> {
        if token.table() != Some(TableId::TypeSpec) {
            return None;
        }
        let assembly = self.frame_ref().assembly;
        let sig = self.loader.assembly(assembly).type_specs.get(&token.row())?;

        match sig {
            TypeSig::MVar(index) => {
                let method = self.frame_ref().method;
                self.loader
                    .registry
                    .method(method)
                    .generic_args
                    .get(*index as usize)
                    .copied()
            }
            TypeSig::Var(index) => {
                // The receiver is argument 0 of an instance method.
                let info = self.loader.registry.method(self.frame_ref().method);
                if !info.signature.has_this {
                    // A static method has no receiver, so the answer comes from
                    // the construction the call site named. Still `None` when
                    // the call site named the definition, which is the case
                    // that has to keep refusing.
                    let construction = self.frame_ref().construction?;
                    return self
                        .loader
                        .registry
                        .ty(construction)
                        .generic_args
                        .get(*index as usize)
                        .copied();
                }
                let handle = self.frame_ref().args.first()?.as_handle()?;
                if handle.is_null() {
                    return None;
                }
                let receiver = self.type_of(handle)?;
                self.loader
                    .registry
                    .ty(receiver)
                    .generic_args
                    .get(*index as usize)
                    .copied()
            }
            _ => None,
        }
    }

    pub(super) fn resolve_method(&mut self, token: Token) -> ExecResult<MethodId> {
        let assembly = self.frame_ref().assembly;
        self.loader
            .resolve_method_token(self.loader.assembly(assembly), token)
            .ok_or(ExecutionError::UnresolvedToken { token, context: "method".into() })
    }

    pub(super) fn resolve_field(&mut self, token: Token) -> ExecResult<FieldId> {
        let assembly = self.frame_ref().assembly;
        self.loader
            .resolve_field_token(self.loader.assembly(assembly), token)
            .ok_or(ExecutionError::UnresolvedToken { token, context: "field".into() })
    }

    // -- calls -------------------------------------------------------------------

    /// `calli`: an indirect call through a function pointer.
    ///
    /// The pointer is whatever `ldftn` or `ldvirtftn` pushed — a [`Value::FnPtr`]
    /// naming a method, not a machine address. That is the whole reason this is
    /// short: the hard part of an indirect call on a real runtime is deciding
    /// what a raw address refers to, and here the answer was never thrown away.
    ///
    /// The operand is a `StandAloneSig` describing the *call site*, not the
    /// callee. It is what says how many arguments to pop, and it can disagree
    /// with the callee's own signature — a mismatch is a malformed program, and
    /// reporting it is better than popping the wrong number of values and
    /// corrupting the evaluation stack underneath.
    fn do_calli(&mut self, token: Token) -> ExecResult<StepOutcome> {
        let site = self.call_site_signature(token)?;
        let arg_count = site.params.len() + usize::from(site.has_this);

        let pointer = self.pop()?;
        let Value::FnPtr(method) = pointer else {
            // A function pointer here names a *method*, not an address, which
            // is what makes `calli` possible at all without a code map. The
            // cost is that it does not survive being stored somewhere shaped
            // like an integer: an element of a `delegate*<...>[]` is an
            // `IntPtr` slot, and putting a method identity into one loses it.
            // Saying so beats "not a function pointer", which invites the
            // reader to go looking at the call site.
            return Err(ExecutionError::exception(
                ClrExceptionKind::InvalidOperation,
                format!(
                    "calli received {pointer:?} rather than a function pointer. A function pointer in this runtime names a method rather than an address, so it does not survive a round trip through integer-shaped storage such as an array element or an `nint`."
                ),
            ));
        };

        let expected = self.loader.registry.method(method).arg_count();
        if expected != arg_count {
            return Err(ExecutionError::InvalidProgram(format!(
                "calli site takes {arg_count} argument(s) but {} takes {expected}",
                self.loader.registry.method(method).qualified_name
            )));
        }

        let mut args = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            args.push(self.pop()?);
        }
        args.reverse();

        match self.enter(method, args)? {
            Entered::Native(value) => {
                if let Some(v) = value {
                    self.push(v);
                }
                Ok(StepOutcome::Continue)
            }
            Entered::Frame => Ok(StepOutcome::Continue),
        }
    }

    /// The signature a `calli` operand names.
    fn call_site_signature(&self, token: Token) -> ExecResult<rustclr_metadata::MethodSig> {
        let assembly = self.frame_ref().assembly;
        self.loader
            .call_site_signature(self.loader.assembly(assembly), token)
            .ok_or(ExecutionError::UnresolvedToken {
                token,
                context: "calli call-site signature".into(),
            })
    }

    fn do_call(
        &mut self,
        token: Token,
        virtual_call: bool,
        constrained: Option<Token>,
    ) -> ExecResult<StepOutcome> {
        // Read before entering: these ask the *calling* frame which assembly
        // the token belongs to.
        let construction = self.constructed_owner(token);
        let call_site_args = self.call_site_type_arguments(token);

        let declared = self.resolve_method(token)?;
        let info = self.loader.registry.method(declared);
        let arg_count = info.arg_count();
        let has_this = info.signature.has_this;

        let mut args = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            args.push(self.pop()?);
        }
        args.reverse();

        // `constrained.` hands the call a managed pointer to the receiver, and
        // ECMA-335 III.2.1 says what to do with it: box a value type so the
        // virtual call reaches the boxed instance, and *dereference* a
        // reference type so `this` is the reference the pointer holds.
        //
        // Only the value-type half used to be implemented, which left every
        // `foreach` broken at the `Dispose` in its finally block: the receiver
        // arrived as a pointer, so dispatch had no object to look at.
        if let Some(ct) = constrained {
            let type_id = self.resolve_type(ct)?;
            let value_like = self.loader.registry.ty(type_id).kind.is_value_like();
            if let Some(first) = args.first_mut() {
                if let Value::Ref(r) = first.clone() {
                    let inner = self.load_indirect(r)?;
                    *first = if value_like {
                        Value::Obj(self.box_value(type_id, inner))
                    } else {
                        inner
                    };
                }
            }
        }

        let target = if virtual_call && has_this {
            match args.first().and_then(|v| v.as_handle()) {
                Some(h) if !h.is_null() => self.resolve_virtual_target(h, declared)?,
                Some(_) => return Err(ExecutionError::null_reference()),
                None => declared,
            }
        } else {
            declared
        };

        if has_this && args.first().is_some_and(|v| v.is_null()) {
            return Err(ExecutionError::null_reference());
        }

        // A value type's method takes an unboxed `this`. Dispatch may have
        // arrived here through a box — `object.ToString()` on a boxed `int`
        // resolves to `Int32::ToString` — so unwrap it, exactly as the CLR
        // does when it calls a value type's method on a boxed instance.
        if has_this {
            let target_type = self.loader.registry.method(target).declaring_type;
            if self.loader.registry.ty(target_type).kind.is_value_like() {
                match args.first().cloned() {
                    Some(Value::Obj(h)) => {
                        if let Some(inner) = self.heap.with::<ClrBox, _>(h, |b| b.value.clone()) {
                            args[0] = inner;
                        }
                    }
                    // A raw pointer used as a value type's receiver. `p[0] + ","`
                    // compiles to `Int32::ToString()` with the pointer *as*
                    // `this`, so without this the method reads the pointer
                    // itself and every such value renders as zero.
                    //
                    // The width comes from the declaring type, which is the only
                    // place it is written down: the pointer does not carry one.
                    Some(Value::Ptr(ptr)) => {
                        let size = self.size_of(target_type);
                        args[0] = self.load_pointer_sized(ptr, size)?;
                    }
                    _ => {}
                }
            }
        }

        self.stage_type_arguments(call_site_args);
        match self.enter(target, args)? {
            Entered::Frame => {
                if construction.is_some() {
                    self.frame().construction = construction;
                }
                Ok(StepOutcome::Continue)
            }
            Entered::Native(Some(v)) => {
                self.push(v);
                Ok(StepOutcome::Continue)
            }
            Entered::Native(None) => Ok(StepOutcome::Continue),
        }
    }

    /// True when a type token names a generic parameter rather than a type
    /// this runtime can identify.
    fn token_names_a_generic_parameter(&self, token: Token) -> bool {
        if token.table() != Some(TableId::TypeSpec) {
            return false;
        }
        let Some(assembly) = self.current_assembly() else { return false };
        matches!(
            self.loader.assembly(assembly).type_specs.get(&token.row()),
            Some(TypeSig::Var(_) | TypeSig::MVar(_))
        )
    }

    /// A native implementation the *receiver's* type provides for `name`.
    ///
    /// Both binding keys are tried, in the same order `enter` uses: the typed
    /// one first, then the arity fallback. Checking only the typed key was a
    /// real bug — `Type::GetCustomAttributes` is registered by arity, so a
    /// virtual call declared on `MemberInfo` reached the base implementation
    /// and read a type id as a method id.
    fn native_override(
        &mut self,
        actual_type: TypeId,
        name: &str,
        sig: &rustclr_metadata::MethodSig,
    ) -> Option<MethodId> {
        let receiver = self.loader.registry.ty(actual_type).full_name();
        for candidate in [
            crate::naming::native_key_typed(&receiver, name, sig),
            crate::naming::native_key(&receiver, name, sig),
        ] {
            if self.natives.contains_key(&candidate) {
                return Some(self.loader.intern_internal_call(
                    actual_type,
                    name,
                    sig.clone(),
                    candidate,
                ));
            }
        }
        None
    }

    /// Picks the most-derived implementation for a virtual call.
    pub(super) fn resolve_virtual_target(
        &mut self,
        receiver: Handle,
        declared: MethodId,
    ) -> ExecResult<MethodId> {
        let actual_type = match self.type_of(receiver) {
            Some(t) => t,
            None => return Ok(declared),
        };
        let info = self.loader.registry.method(declared);
        let declaring = info.declaring_type;

        // Interface dispatch: the declared slot is meaningless on the target,
        // so match by name and shape across the receiver's hierarchy.
        if self.loader.registry.ty(declaring).kind == TypeKind::Interface {
            let name = info.name.clone();
            let sig = info.signature.clone();
            // An explicit implementation is emitted under a mangled name, so it
            // has to be looked up through `MethodImpl` before matching by
            // shape — otherwise it is invisible.
            if let Some(m) = self.loader.explicit_implementation(actual_type, declared) {
                return Ok(m);
            }
            if let Some(m) = self.loader.find_method_on_type(actual_type, &name, &sig) {
                return Ok(m);
            }
            // A natively implemented receiver has no managed method to find —
            // `List<T>` carries no IL at all. Bind to its native
            // implementation instead, so `IEnumerable<T>::GetEnumerator` on a
            // list reaches the list's own enumerator rather than the
            // interface's unimplemented stub.
            if let Some(m) = self.native_override(actual_type, &name, &sig) {
                return Ok(m);
            }
            return Ok(declared);
        }

        match info.vtable_slot {
            Some(slot) => Ok(self
                .loader
                .registry
                .resolve_virtual(actual_type, slot)
                .unwrap_or(declared)),
            // No slot means the declared method is a native stub — the
            // framework's own `Object::ToString`, `Equals` or `GetHashCode`,
            // which have no managed body and so were never laid out in a
            // vtable. A `callvirt` at one still has to reach the receiver's
            // override: `item.ToString()` compiles to a virtual call on
            // `System.Object::ToString`, and without this it printed the type
            // name instead of running the user's method.
            //
            // Only managed methods are found here — native stubs are not
            // recorded on their declaring type — so a native implementation is
            // never shadowed by this.
            None => {
                let name = info.name.clone();
                let sig = info.signature.clone();
                if let Some(m) = self.loader.find_method_on_type(actual_type, &name, &sig) {
                    return Ok(m);
                }
                // The receiver's own native implementation wins over the one
                // declared on its base. `System.Type` derives from
                // `MemberInfo`, so `typeof(T).Name` is a virtual call on
                // `MemberInfo::get_Name` — and answering it with the base's
                // implementation would read a type id as a method id.
                if actual_type != declaring {
                    if let Some(m) = self.native_override(actual_type, &name, &sig) {
                        return Ok(m);
                    }
                }
                Ok(declared)
            }
        }
    }

    fn do_newobj(&mut self, token: Token) -> ExecResult<StepOutcome> {
        // `new Span<int>(ptr, 4)` is a `newobj`, and the width of an element
        // is only in the `int`. Staged the same way a `call` stages it.
        let call_site_args = self.call_site_type_arguments(token);
        let ctor = self.resolve_method(token)?;
        let info = self.loader.registry.method(ctor);
        let param_count = info.signature.params.len();
        // A constructor on a closed construction resolves to the *definition's*
        // method, because one body serves every construction — so the method's
        // declaring type is the open definition and would give the new instance
        // a type whose `T` is unknowable. The member reference remembers which
        // construction was named; prefer it.
        let type_id = self
            .constructed_owner(token)
            .unwrap_or_else(|| self.loader.registry.method(ctor).declaring_type);

        let mut args = Vec::with_capacity(param_count + 1);
        for _ in 0..param_count {
            args.push(self.pop()?);
        }
        args.reverse();

        let kind = self.loader.registry.ty(type_id).kind;

        // Delegates are constructed by the runtime from (target, method).
        if kind == TypeKind::Delegate {
            let method = match args.get(1) {
                Some(Value::FnPtr(m)) => *m,
                _ => {
                    return Err(ExecutionError::InvalidProgram(
                        "delegate constructor expects a method pointer".into(),
                    ))
                }
            };
            let receiver = args.first().and_then(|v| v.as_handle()).unwrap_or(Handle::NULL);
            self.stats.allocations += 1;
            let h = self.heap.alloc(ClrDelegate {
                type_id,
                targets: vec![DelegateTarget { receiver, method }],
            });
            self.push(Value::Obj(h));
            return Ok(StepOutcome::Continue);
        }

        if kind.is_value_like() {
            // `this` in a value-type constructor is a managed pointer, and
            // `newobj` has no slot to point at yet. Allocate a one-field cell
            // to serve as that slot: the constructor writes through it, and
            // `do_return` hands the caller the field rather than the cell.
            //
            // Without this, a `record struct` or any struct with a real
            // constructor would silently yield its zero value.
            let zero = self.zero_of(type_id);
            let object_type = self.loader.core().object;
            let mut cell = ClrObject::new(object_type, 1);
            cell.fields[0] = zero;
            self.stats.allocations += 1;
            let cell = self.heap.alloc(cell);

            let mut call_args = Vec::with_capacity(param_count + 1);
            call_args.push(Value::Ref(ByRef::Field { object: cell, slot: 0 }));
            call_args.extend(args);

            self.stage_type_arguments(call_site_args.clone());
            self.stage_type_arguments(call_site_args);
        match self.enter(ctor, call_args)? {
                Entered::Frame => {
                    let frame = self.frames.last_mut().expect("ctor frame");
                    frame.pending_newobj = Some(cell);
                    frame.pending_newobj_is_cell = true;
                    return Ok(StepOutcome::Continue);
                }
                // A native constructor either returned the value directly or
                // wrote it into the cell.
                Entered::Native(Some(v)) => {
                    self.push(v);
                    return Ok(StepOutcome::Continue);
                }
                Entered::Native(None) => {
                    let constructed = self
                        .heap.with::<ClrObject, _>(cell, |o| o.fields.first().cloned()).flatten()
                        .unwrap_or_else(|| self.zero_of(type_id));
                    self.push(constructed);
                    return Ok(StepOutcome::Continue);
                }
            }
        }

        self.ensure_cctor(type_id)?;

        // Collect before allocating. Between `alloc_object` and the frame that
        // carries the instance there is no root pointing at it, so collecting
        // there reclaims the object under construction — which then surfaces as
        // a stale handle on the first field access.
        self.maybe_collect();
        let handle = self.alloc_object(type_id);

        let mut call_args = Vec::with_capacity(param_count + 1);
        call_args.push(Value::Obj(handle));
        call_args.extend(args);

        self.stage_type_arguments(call_site_args);
        match self.enter(ctor, call_args)? {
            Entered::Frame => {
                // The constructor returns void; the new object must end up on
                // the caller's stack. Record it so `do_return` can push it.
                self.frames.last_mut().expect("ctor frame").pending_newobj = Some(handle);
                Ok(StepOutcome::Continue)
            }
            Entered::Native(_) => {
                self.push(Value::Obj(handle));
                Ok(StepOutcome::Continue)
            }
        }
    }

    // -- fields ------------------------------------------------------------------

    fn load_field(&mut self, obj: &Value, token: Token) -> ExecResult<Value> {
        let field = self.resolve_field(token)?;
        if self.loader.registry.field(field).is_static {
            self.ensure_cctor(self.loader.registry.field(field).declaring_type)?;
            return Ok(self.loader.static_value(field).clone());
        }

        match obj {
            Value::Obj(h) if !h.is_null() => {
                let type_id = self.type_of(*h).unwrap_or(TypeId::INVALID);
                let slot = self.field_slot(type_id, field).ok_or_else(|| self.missing_field(field))?;
                self.heap.with::<ClrObject, _>(*h, |o| o.fields.get(slot).cloned()).flatten()
                    .ok_or_else(|| self.missing_field(field))
            }
            Value::Ref(r) => {
                let inner = self.load_indirect(r.clone())?;
                self.load_field(&inner, token)
            }
            Value::Struct(s) => {
                let slot = self.field_slot(s.type_id, field).unwrap_or(0);
                Ok(s.fields.get(slot).cloned().unwrap_or(Value::Null))
            }
            _ => Err(ExecutionError::null_reference()),
        }
    }

    fn store_field(&mut self, obj: &Value, token: Token, value: Value) -> ExecResult<()> {
        let field = self.resolve_field(token)?;
        if self.loader.registry.field(field).is_static {
            self.ensure_cctor(self.loader.registry.field(field).declaring_type)?;
            self.loader.set_static(field, value);
            return Ok(());
        }

        // A struct is assigned through a managed pointer: read it, update the
        // slot, write it back. C# reaches every field of a local struct this
        // way (`ldloca`; `stfld`).
        if let Value::Ref(r) = obj {
            let mut current = self.load_indirect(r.clone())?;
            self.set_struct_field(&mut current, field, value)?;
            return self.store_indirect(r.clone(), current);
        }

        let h = obj.as_handle().filter(|h| !h.is_null()).ok_or_else(
            ExecutionError::null_reference,
        )?;
        let type_id = self.type_of(h).unwrap_or(TypeId::INVALID);
        let slot = self.field_slot(type_id, field).ok_or_else(|| self.missing_field(field))?;
        let stored = self.heap.with_mut::<ClrObject, _>(h, |o| {
            if slot < o.fields.len() {
                o.fields[slot] = value;
                true
            } else {
                false
            }
        });
        match stored {
            Some(true) => Ok(()),
            _ => Err(self.missing_field(field)),
        }
    }

    /// Writes one field of an unboxed value-type instance in place.
    ///
    /// A struct whose zero value collapsed to a scalar (because its layout was
    /// not known when the local was created) is promoted to a full
    /// [`StructValue`] on first field write, so the remaining fields survive.
    fn set_struct_field(
        &mut self,
        target: &mut Value,
        field: FieldId,
        value: Value,
    ) -> ExecResult<()> {
        let declaring = self.loader.registry.field(field).declaring_type;
        let slot = self
            .field_slot(declaring, field)
            .ok_or_else(|| self.missing_field(field))?;

        if !matches!(target, Value::Struct(_)) {
            *target = self.zero_of(declaring);
        }

        match target {
            Value::Struct(s) => {
                if s.fields.len() <= slot {
                    s.fields.resize(slot + 1, Value::Null);
                }
                s.fields[slot] = value;
                Ok(())
            }
            // A struct with no fields has nothing to write.
            _ => Ok(()),
        }
    }


    // -- raw pointers --------------------------------------------------------
    //
    // A raw pointer is a buffer plus a byte offset. Reading or writing through
    // one has to land on the right element of that buffer, which means turning
    // the byte offset back into an index using the storage's element width.
    // Aligned access is all C# emits for `int*` over `int[]` or over
    // `stackalloc int[n]`, and an unaligned one is refused rather than
    // silently reading across two elements.

    /// The byte buffer `stackalloc` gets.
    ///
    /// A `byte[]`, so `element_width` is one and offsets are indices. Living
    /// on the managed heap rather than the native stack is what makes it safe:
    /// the pointer roots it, so it outlives every reference to it.
    pub(super) fn alloc_byte_buffer(&mut self, size: usize) -> Handle {
        let element_type = self.loader.primitive_type(crate::types::Primitive::Byte);
        let array_type = self
            .loader
            .registry
            .find_sz_array(element_type)
            .unwrap_or_else(|| self.loader.core().array);
        self.stats.allocations += 1;
        self.heap.alloc(ClrArray {
            array_type,
            element_type,
            storage: ArrayStorage::U8(vec![0u8; size]),
            dimensions: vec![size as u32],
        })
    }

    /// How many bytes an indirect instruction touches.
    ///
    /// The width is in the opcode, not in the pointer — `int* p` and `byte* q`
    /// are the same value here, and `*p` versus `*q` is the difference between
    /// `ldind.i4` and `ldind.u1`. Reading it from the buffer instead truncated
    /// every `stackalloc int[]` write to one byte, which happened to be
    /// invisible for values below 256.
    fn access_width(op: Op) -> Option<(usize, bool)> {
        Some(match op {
            Op::LdindI1 | Op::StindI1 => (1, true),
            Op::LdindU1 => (1, false),
            Op::LdindI2 | Op::StindI2 => (2, true),
            Op::LdindU2 => (2, false),
            Op::LdindI4 | Op::StindI4 | Op::LdindR4 | Op::StindR4 => (4, true),
            Op::LdindU4 => (4, false),
            Op::LdindI8 | Op::StindI8 | Op::LdindR8 | Op::StindR8 => (8, true),
            Op::LdindI | Op::StindI => (8, true),
            _ => return None,
        })
    }

    /// The buffer behind a pointer, and how wide its elements are.
    fn pointer_buffer(&mut self, p: RawPtr) -> ExecResult<(Handle, usize)> {
        if p.is_null() {
            return Err(ExecutionError::null_reference());
        }
        let width = self
            .heap
            .with::<ClrArray, _>(p.buffer, |a| a.storage.element_width())
            .flatten()
            .ok_or_else(|| {
                ExecutionError::Unsupported(
                    "dereferencing a pointer into storage that is not laid out in bytes".into(),
                )
            })?;
        if p.offset < 0 {
            return Err(ExecutionError::exception(
                ClrExceptionKind::IndexOutOfRange,
                "a pointer read before the start of its buffer",
            ));
        }
        Ok((p.buffer, width))
    }

    /// Reads through a pointer at a width taken from a type rather than an
    /// opcode.
    pub(super) fn load_pointer_sized(&mut self, p: RawPtr, size: usize) -> ExecResult<Value> {
        let op = match size {
            1 => Op::LdindI1,
            2 => Op::LdindI2,
            8 => Op::LdindI8,
            _ => Op::LdindI4,
        };
        self.load_through_pointer(p, op)
    }

    pub(super) fn load_through_pointer(&mut self, p: RawPtr, op: Op) -> ExecResult<Value> {
        let (buffer, element) = self.pointer_buffer(p)?;
        let (width, signed) = Self::access_width(op).unwrap_or((element, true));

        // Reading exactly one element of a typed array — what `fixed` over an
        // `int[]` does — reads the value, not its bytes. The array holds typed
        // values here rather than a byte image, so there is nothing else it
        // could honestly mean.
        if element == width {
            let index = (p.offset / element as i64) as usize;
            if p.offset % element as i64 != 0 {
                return Err(unaligned(p.offset, element));
            }
            return self
                .heap
                .with::<ClrArray, _>(buffer, |a| a.storage.get(index))
                .flatten()
                .ok_or_else(|| past_the_end("read"));
        }

        // Otherwise the buffer must be bytes — `stackalloc` memory — and the
        // value is assembled from `width` of them, little-endian.
        if element != 1 {
            return Err(ExecutionError::Unsupported(format!(
                "a {width}-byte read through a pointer into {element}-byte elements"
            )));
        }
        let bytes = self.read_bytes(p, width)?;
        let mut raw = 0u64;
        for (n, b) in bytes.iter().enumerate() {
            raw |= (*b as u64) << (8 * n);
        }
        Ok(match (width, signed, op) {
            (4, _, Op::LdindR4) => Value::F(f32::from_bits(raw as u32) as f64),
            (8, _, Op::LdindR8) => Value::F(f64::from_bits(raw)),
            (1, true, _) => Value::I32(raw as u8 as i8 as i32),
            (1, false, _) => Value::I32(raw as u8 as i32),
            (2, true, _) => Value::I32(raw as u16 as i16 as i32),
            (2, false, _) => Value::I32(raw as u16 as i32),
            (4, true, _) => Value::I32(raw as u32 as i32),
            (4, false, _) => Value::I32(raw as u32 as i32),
            _ => Value::I64(raw as i64),
        })
    }

    pub(super) fn store_through_pointer(
        &mut self,
        p: RawPtr,
        value: Value,
        op: Op,
    ) -> ExecResult<()> {
        let (buffer, element) = self.pointer_buffer(p)?;
        let (width, _) = Self::access_width(op).unwrap_or((element, true));

        if element == width {
            if p.offset % element as i64 != 0 {
                return Err(unaligned(p.offset, element));
            }
            let index = (p.offset / element as i64) as usize;
            let stored = self
                .heap
                .with_mut::<ClrArray, _>(buffer, |a| {
                    if index < a.storage.len() {
                        a.storage.set(index, &value);
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            return if stored { Ok(()) } else { Err(past_the_end("write")) };
        }

        if element != 1 {
            return Err(ExecutionError::Unsupported(format!(
                "a {width}-byte write through a pointer into {element}-byte elements"
            )));
        }
        let raw = match (width, op) {
            (4, Op::StindR4) => (value.as_f64().unwrap_or(0.0) as f32).to_bits() as u64,
            (8, Op::StindR8) => value.as_f64().unwrap_or(0.0).to_bits(),
            _ => value.as_i64().unwrap_or(0) as u64,
        };
        for n in 0..width {
            self.store_byte(p.offset_by(n as i64), (raw >> (8 * n)) as u8)?;
        }
        Ok(())
    }

    /// Writes one byte of a byte buffer.
    fn store_byte(&mut self, p: RawPtr, byte: u8) -> ExecResult<()> {
        let index = p.offset as usize;
        let stored = self
            .heap
            .with_mut::<ClrArray, _>(p.buffer, |a| {
                if index < a.storage.len() {
                    a.storage.set(index, &Value::I32(byte as i32));
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if stored {
            Ok(())
        } else {
            Err(past_the_end("write"))
        }
    }

    /// `cpblk`: copy `count` bytes from one pointer to another.
    ///
    /// Both sides must be byte buffers. Copying between arrays of wider
    /// elements would mean reinterpreting their bytes, and this runtime holds
    /// them as typed values rather than a byte image, so there is nothing to
    /// reinterpret — it refuses instead of producing something plausible.
    pub(super) fn copy_block(&mut self, to: Value, from: Value, count: usize) -> ExecResult<()> {
        let (Value::Ptr(to), Value::Ptr(from)) = (to, from) else {
            return Err(ExecutionError::Unsupported(
                "`cpblk` between anything other than two raw pointers".into(),
            ));
        };
        let bytes = self.read_bytes(from, count)?;
        for (n, byte) in bytes.into_iter().enumerate() {
            self.store_byte(to.offset_by(n as i64), byte)?;
        }
        Ok(())
    }

    /// `initblk`: set `count` bytes to `fill`.
    pub(super) fn fill_block(&mut self, to: Value, fill: u8, count: usize) -> ExecResult<()> {
        let Value::Ptr(to) = to else {
            return Err(ExecutionError::Unsupported(
                "`initblk` on anything other than a raw pointer".into(),
            ));
        };
        for n in 0..count {
            self.store_byte(to.offset_by(n as i64), fill)?;
        }
        Ok(())
    }

    fn read_bytes(&mut self, from: RawPtr, count: usize) -> ExecResult<Vec<u8>> {
        let mut out = Vec::with_capacity(count);
        for n in 0..count {
            let v = self.load_through_pointer(from.offset_by(n as i64), Op::LdindU1)?;
            out.push(v.as_i32().unwrap_or(0) as u8);
        }
        Ok(out)
    }

    // -- indirect access -----------------------------------------------------------

    pub(super) fn load_indirect(&mut self, r: ByRef) -> ExecResult<Value> {
        Ok(match r {
            ByRef::Local { frame, index } => self
                .frames
                .iter()
                .find(|f| f.id == frame)
                .and_then(|f| f.locals.get(index as usize).cloned())
                .unwrap_or(Value::Null),
            ByRef::Arg { frame, index } => self
                .frames
                .iter()
                .find(|f| f.id == frame)
                .and_then(|f| f.args.get(index as usize).cloned())
                .unwrap_or(Value::Null),
            ByRef::Field { object, slot } => self
                .heap.with::<ClrObject, _>(object, |o| o.fields.get(slot as usize).cloned()).flatten()
                .ok_or_else(ExecutionError::null_reference)?,
            ByRef::Static { slot, .. } => self.loader.static_value(FieldId(slot)).clone(),
            ByRef::ArrayElement { array, index } => self
                .heap.with::<ClrArray, _>(array, |a| a.storage.get(index as usize)).flatten()
                .ok_or_else(|| ExecutionError::index_out_of_range(index as i64, 0))?,
            ByRef::StructField { base, slot } => {
                let container = self.load_indirect(*base)?;
                match container {
                    Value::Struct(s) => s.fields.get(slot as usize).cloned().unwrap_or(Value::Null),
                    // The container is not a struct any more, which means the
                    // slot the pointer was taken from has been overwritten.
                    other => return Err(self.type_error("load through a struct field pointer", &other)),
                }
            }
        })
    }

    pub(super) fn store_indirect(&mut self, r: ByRef, value: Value) -> ExecResult<()> {
        match r {
            ByRef::Local { frame, index } => {
                if let Some(f) = self.frames.iter_mut().find(|f| f.id == frame) {
                    if (index as usize) < f.locals.len() {
                        f.locals[index as usize] = value;
                    }
                }
            }
            ByRef::Arg { frame, index } => {
                if let Some(f) = self.frames.iter_mut().find(|f| f.id == frame) {
                    if (index as usize) < f.args.len() {
                        f.args[index as usize] = value;
                    }
                }
            }
            ByRef::Field { object, slot } => {
                let stored = self.heap.with_mut::<ClrObject, _>(object, |o| {
                    let at = slot as usize;
                    if at < o.fields.len() {
                        o.fields[at] = value;
                        true
                    } else {
                        false
                    }
                });
                if stored != Some(true) {
                    return Err(ExecutionError::null_reference());
                }
            }
            ByRef::Static { slot, .. } => {
                self.loader.set_static(FieldId(slot), value);
            }
            ByRef::StructField { base, slot } => {
                // Value types are copied, not aliased: read the container,
                // write the field, write the container back through the same
                // pointer. The base always resolves to the live slot, so the
                // update lands where the struct actually lives.
                let mut container = self.load_indirect((*base).clone())?;
                match &mut container {
                    Value::Struct(s) => {
                        if let Some(f) = s.fields.get_mut(slot as usize) {
                            *f = value;
                        }
                    }
                    other => {
                        return Err(
                            self.type_error("store through a struct field pointer", &other.clone())
                        )
                    }
                }
                return self.store_indirect(*base, container);
            }
            ByRef::ArrayElement { array, index } => {
                let ok = self
                    .heap.with_mut::<ClrArray, _>(array, |a| a.storage.set(index as usize, &value))
                    .unwrap_or(false);
                if !ok {
                    return Err(ExecutionError::index_out_of_range(index as i64, 0));
                }
            }
        }
        Ok(())
    }

    fn narrow_for_ldind(&self, op: Op, v: Value) -> Value {
        let Some(i) = v.as_i32() else { return v };
        match op {
            Op::LdindI1 => Value::I32(i as i8 as i32),
            Op::LdindU1 => Value::I32(i as u8 as i32),
            Op::LdindI2 => Value::I32(i as i16 as i32),
            Op::LdindU2 => Value::I32(i as u16 as i32),
            _ => v,
        }
    }

    fn narrow_for_stind(&self, op: Op, v: Value) -> Value {
        let Some(i) = v.as_i32() else { return v };
        match op {
            Op::StindI1 => Value::I32(i as i8 as i32),
            Op::StindI2 => Value::I32(i as i16 as i32),
            _ => v,
        }
    }

    // -- arrays --------------------------------------------------------------------

    fn load_element(&mut self, array: &Value, index: &Value) -> ExecResult<Value> {
        let h = array.as_handle().filter(|h| !h.is_null()).ok_or_else(
            ExecutionError::null_reference,
        )?;
        let i = index.as_i64().unwrap_or(-1);
        let len = self.heap.with::<ClrArray, _>(h, |a| a.len()).unwrap_or(0);
        if i < 0 || i as usize >= len {
            return Err(ExecutionError::index_out_of_range(i, len));
        }
        self.heap.with::<ClrArray, _>(h, |a| a.storage.get(i as usize)).flatten()
            .ok_or_else(|| ExecutionError::index_out_of_range(i, len))
    }

    fn store_element(&mut self, array: &Value, index: &Value, value: Value) -> ExecResult<()> {
        let h = array.as_handle().filter(|h| !h.is_null()).ok_or_else(
            ExecutionError::null_reference,
        )?;
        let i = index.as_i64().unwrap_or(-1);
        let len = self.heap.with::<ClrArray, _>(h, |a| a.len()).unwrap_or(0);
        if i < 0 || i as usize >= len {
            return Err(ExecutionError::index_out_of_range(i, len));
        }
        self.heap.with_mut::<ClrArray, _>(h, |a| a.storage.set(i as usize, &value));
        Ok(())
    }

    // -- boxing --------------------------------------------------------------------

    fn unbox(&mut self, v: Value, target: TypeId, any: bool) -> ExecResult<Value> {
        match v.as_handle() {
            Some(h) if h.is_null() => Err(ExecutionError::null_reference()),
            Some(h) => {
                if let Some((inner, boxed_type)) =
                    self.heap.with::<ClrBox, _>(h, |b| (b.value.clone(), b.type_id))
                {
                    if boxed_type == target
                        || self.loader.registry.is_assignable_to(boxed_type, target)
                    {
                        return Ok(inner);
                    }
                    let from = self.loader.registry.ty(boxed_type).full_name();
                    let to = self.loader.registry.ty(target).full_name();
                    return Err(ExecutionError::invalid_cast(&from, &to));
                }
                if any {
                    // `unbox.any` on a reference type is a cast.
                    let actual = self.type_of(h).unwrap_or(TypeId::INVALID);
                    if self.loader.registry.is_assignable_to(actual, target) {
                        return Ok(Value::Obj(h));
                    }
                    let from = self.type_name_of(h);
                    let to = self.loader.registry.ty(target).full_name();
                    return Err(ExecutionError::invalid_cast(&from, &to));
                }
                Err(ExecutionError::invalid_cast("object", &self.loader.registry.ty(target).full_name()))
            }
            None => Ok(v),
        }
    }

    /// The zero value of a type, for `initobj` and default locals.
    /// The default value of a type: zeroed fields for a struct, null for a
    /// reference. Public because `Activator.CreateInstance` needs it too.
    pub fn zero_of(&self, type_id: TypeId) -> Value {
        let ty = self.loader.registry.ty(type_id);
        match ty.primitive {
            Some(Primitive::Int64) | Some(Primitive::UInt64) => Value::I64(0),
            Some(Primitive::Single) | Some(Primitive::Double) => Value::F(0.0),
            Some(Primitive::IntPtr) | Some(Primitive::UIntPtr) => Value::NativeInt(0),
            Some(_) => Value::I32(0),
            None if ty.kind.is_value_like() => {
                let fields = self.instance_fields(type_id);
                if fields.is_empty() {
                    Value::I32(0)
                } else {
                    Value::Struct(Box::new(StructValue {
                        type_id,
                        fields: fields
                            .iter()
                            .map(|f| {
                                self.default_value_for(&self.loader.registry.field(*f).signature)
                            })
                            .collect(),
                    }))
                }
            }
            None => Value::Null,
        }
    }

    /// Storage size of a type, for `sizeof`.
    pub(super) fn size_of(&self, type_id: TypeId) -> usize {
        let ty = self.loader.registry.ty(type_id);
        if let Some(p) = ty.primitive {
            return p.size();
        }
        if let Some(size) = ty.explicit_size {
            return size as usize;
        }
        if ty.kind.is_value_like() {
            return self
                .instance_fields(type_id)
                .iter()
                .map(|f| {
                    let sig = &self.loader.registry.field(*f).signature;
                    match sig.unwrap_modifiers() {
                        TypeSig::I8 | TypeSig::U8 | TypeSig::R8 => 8,
                        TypeSig::I2 | TypeSig::U2 | TypeSig::Char => 2,
                        TypeSig::I1 | TypeSig::U1 | TypeSig::Boolean => 1,
                        _ => 4,
                    }
                })
                .sum::<usize>()
                .max(1);
        }
        core::mem::size_of::<usize>()
    }
}

/// A pointer whose offset does not land on an element boundary.
fn unaligned(offset: i64, width: usize) -> ExecutionError {
    ExecutionError::Unsupported(alloc::format!(
        "an unaligned pointer: byte {offset} into {width}-byte elements"
    ))
}

fn past_the_end(what: &str) -> ExecutionError {
    ExecutionError::exception(
        ClrExceptionKind::IndexOutOfRange,
        alloc::format!("a pointer {what} past the end of its buffer"),
    )
}
