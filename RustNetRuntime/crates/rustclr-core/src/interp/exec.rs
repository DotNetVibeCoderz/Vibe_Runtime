//! Instruction dispatch.
//!
//! One `match` over [`Op`] covering the IL subset RustCLR executes. Numeric
//! behaviour follows ECMA-335 III.1.5 (binary numeric promotion) and III.3
//! (per-instruction semantics); where this runtime deliberately narrows the
//! spec, the arm says so.

use super::*;

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
                let r = self.compare_ordered(&a, &b, false)? == Some(std::cmp::Ordering::Greater);
                self.push(Value::I32(r as i32));
            }
            Op::CgtUn => {
                let (a, b) = self.pop2()?;
                // `cgt.un` on floats is true when unordered, which is how
                // `!(a <= b)` is compiled.
                let r = match self.compare_ordered(&a, &b, true)? {
                    Some(std::cmp::Ordering::Greater) => true,
                    None => true,
                    _ => false,
                };
                self.push(Value::I32(r as i32));
            }
            Op::Clt => {
                let (a, b) = self.pop2()?;
                let r = self.compare_ordered(&a, &b, false)? == Some(std::cmp::Ordering::Less);
                self.push(Value::I32(r as i32));
            }
            Op::CltUn => {
                let (a, b) = self.pop2()?;
                let r = match self.compare_ordered(&a, &b, true)? {
                    Some(std::cmp::Ordering::Less) => true,
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
                *self.loader.static_value_mut(field) = v;
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
                let r = addr.as_byref().ok_or_else(ExecutionError::null_reference)?;
                let v = self.load_indirect(r)?;
                self.push(self.narrow_for_ldind(ins.op, v));
            }
            Op::StindI1 | Op::StindI2 | Op::StindI4 | Op::StindI8 | Op::StindI | Op::StindR4
            | Op::StindR8 | Op::StindRef => {
                let value = self.pop()?;
                let addr = self.pop()?;
                let r = addr.as_byref().ok_or_else(ExecutionError::null_reference)?;
                let narrowed = self.narrow_for_stind(ins.op, value);
                self.store_indirect(r, narrowed)?;
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
                    .heap
                    .get_as::<ClrArray>(h)
                    .map(|a| a.len())
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
                let len = self.heap.get_as::<ClrArray>(h).map_or(0, |a| a.len());
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
                // Filters are evaluated by `dispatch_exception`; reaching this
                // instruction during normal flow means the filter fell through.
                let _ = self.pop()?;
            }

            // -- metadata ------------------------------------------------------------------------
            Op::Ldtoken => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                // A handle is opaque to managed code; carry the raw token. That
                // is enough for `RuntimeHelpers.InitializeArray` and for the
                // identity comparisons a record generates, and no more — see
                // "Reflection is minimal" in docs/limitations.md.
                self.push(Value::I32(token.raw() as i32));
            }
            Op::Sizeof => {
                let token = ins.operand.as_token().ok_or_else(|| self.bad_operand(ins))?;
                let type_id = self.resolve_type(token)?;
                self.push(Value::I32(self.size_of(type_id) as i32));
            }

            // -- deliberately unsupported ---------------------------------------------------------
            Op::Localloc
            | Op::Cpblk
            | Op::Initblk
            | Op::Arglist
            | Op::Mkrefany
            | Op::Refanyval
            | Op::Refanytype
            | Op::Calli
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
        let assembly = self.frame_ref().assembly;
        self.loader
            .resolve_type_token(self.loader.assembly(assembly), token)
            .ok_or(ExecutionError::UnresolvedToken { token, context: "type".into() })
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

    fn do_call(
        &mut self,
        token: Token,
        virtual_call: bool,
        constrained: Option<Token>,
    ) -> ExecResult<StepOutcome> {
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

        match self.enter(target, args)? {
            Entered::Frame => Ok(StepOutcome::Continue),
            Entered::Native(Some(v)) => {
                self.push(v);
                Ok(StepOutcome::Continue)
            }
            Entered::Native(None) => Ok(StepOutcome::Continue),
        }
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
            let receiver_name = self.loader.registry.ty(actual_type).full_name();
            let native = crate::naming::native_key_typed(&receiver_name, &name, &sig);
            if self.natives.contains_key(&native) {
                return Ok(self.loader.intern_internal_call(actual_type, &name, sig, native));
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
                Ok(self
                    .loader
                    .find_method_on_type(actual_type, &name, &sig)
                    .unwrap_or(declared))
            }
        }
    }

    fn do_newobj(&mut self, token: Token) -> ExecResult<StepOutcome> {
        let ctor = self.resolve_method(token)?;
        let info = self.loader.registry.method(ctor);
        let type_id = info.declaring_type;
        let param_count = info.signature.params.len();

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
                        .heap
                        .get_as::<ClrObject>(cell)
                        .and_then(|o| o.fields.first().cloned())
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
                self.heap
                    .get_as::<ClrObject>(*h)
                    .and_then(|o| o.fields.get(slot).cloned())
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
            *self.loader.static_value_mut(field) = value;
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
        match self.heap.get_as_mut::<ClrObject>(h) {
            Some(o) if slot < o.fields.len() => {
                o.fields[slot] = value;
                Ok(())
            }
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
                .heap
                .get_as::<ClrObject>(object)
                .and_then(|o| o.fields.get(slot as usize).cloned())
                .ok_or_else(ExecutionError::null_reference)?,
            ByRef::Static { slot, .. } => self.loader.static_value(FieldId(slot)).clone(),
            ByRef::ArrayElement { array, index } => self
                .heap
                .get_as::<ClrArray>(array)
                .and_then(|a| a.storage.get(index as usize))
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
            ByRef::Field { object, slot } => match self.heap.get_as_mut::<ClrObject>(object) {
                Some(o) if (slot as usize) < o.fields.len() => o.fields[slot as usize] = value,
                _ => return Err(ExecutionError::null_reference()),
            },
            ByRef::Static { slot, .. } => {
                *self.loader.static_value_mut(FieldId(slot)) = value;
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
                    .heap
                    .get_as_mut::<ClrArray>(array)
                    .map(|a| a.storage.set(index as usize, &value))
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
        let len = self.heap.get_as::<ClrArray>(h).map_or(0, |a| a.len());
        if i < 0 || i as usize >= len {
            return Err(ExecutionError::index_out_of_range(i, len));
        }
        self.heap
            .get_as::<ClrArray>(h)
            .and_then(|a| a.storage.get(i as usize))
            .ok_or_else(|| ExecutionError::index_out_of_range(i, len))
    }

    fn store_element(&mut self, array: &Value, index: &Value, value: Value) -> ExecResult<()> {
        let h = array.as_handle().filter(|h| !h.is_null()).ok_or_else(
            ExecutionError::null_reference,
        )?;
        let i = index.as_i64().unwrap_or(-1);
        let len = self.heap.get_as::<ClrArray>(h).map_or(0, |a| a.len());
        if i < 0 || i as usize >= len {
            return Err(ExecutionError::index_out_of_range(i, len));
        }
        self.heap
            .get_as_mut::<ClrArray>(h)
            .map(|a| a.storage.set(i as usize, &value));
        Ok(())
    }

    // -- boxing --------------------------------------------------------------------

    fn unbox(&mut self, v: Value, target: TypeId, any: bool) -> ExecResult<Value> {
        match v.as_handle() {
            Some(h) if h.is_null() => Err(ExecutionError::null_reference()),
            Some(h) => {
                if let Some(b) = self.heap.get_as::<ClrBox>(h) {
                    let inner = b.value.clone();
                    let boxed_type = b.type_id;
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
    pub(super) fn zero_of(&self, type_id: TypeId) -> Value {
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
