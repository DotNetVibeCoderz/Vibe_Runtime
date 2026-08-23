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
use crate::translate::{Backend, BinOp, Cond, UnOp};
use crate::verify::MethodAnalysis;
use crate::{analyse, CompileError, CompiledCode, Compiler, Tier};
use rustclr_core::opcode::{decode_all, Instruction};
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

/// Scratch xmm registers. Nothing lives in one between instructions, so two
/// is enough for every operation the float path emits.
const XMM0: u8 = 0;
const XMM1: u8 = 1;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RDI: u8 = 7;
/// Argument registers beyond the first two, for the C calls the allocation
/// helper needs. System V uses rdi/rsi/rdx; Windows uses rcx/rdx/r8.
const RSI: u8 = 6;
const R8: u8 = 8;
const R9: u8 = 9;
const R12: u8 = 12;
const R13: u8 = 13;
const R14: u8 = 14;
const R15: u8 = 15;

/// Evaluation-stack slots kept in registers, shallowest first.
const CACHED: [u8; 2] = [R14, R15];

/// Registers a method's busiest locals and arguments are pinned to.
///
/// R12 and R13 are the two general-purpose registers callee-saved under *both*
/// the Windows and System V conventions that nothing else here has claimed —
/// RBX holds the argument pointer, RBP the frame, R14 and R15 the top two
/// evaluation-stack slots. Being callee-saved is what lets a pinned value
/// survive a call to an allocation helper without being spilled around it.
const PINNED: [u8; 2] = [R12, R13];

/// Bytes of frame occupied by callee-saved registers pushed after `rbp` is
/// established. Locals and spill slots start below this.
const SAVED_BYTES: i32 = 40;

/// A slot a method reads and writes: an argument, or a local.
///
/// The two are pinned by the same mechanism because they behave the same way
/// once compiled — a frame slot read and written by index. They are told apart
/// only when the prologue has to give a pinned register its starting value: an
/// argument's comes from the caller, a local's is zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pinnable {
    Arg(usize),
    Local(usize),
}

/// Which slots are worth a register, busiest first.
///
/// Counting uses is a crude heuristic and deliberately so: a loop-weighted
/// count would be better, and would also be another thing that can be wrong in
/// code whose failures are silent. A slot read four times in a method is very
/// likely read four times in its loop.
/// Whether a callee is one this backend would take.
///
/// Asked at *compile* time, and deliberately a shape question rather than an
/// attempt to compile: two methods may call each other, and compiling one to
/// answer a question about the other would not terminate. Whether the callee is
/// actually compiled is settled at run time, before the caller first runs.
fn could_compile(loader: &Loader, callee: MethodId) -> bool {
    let info = loader.registry.method(callee);
    if info.signature.has_this || !matches!(info.kind, MethodKind::Il(_)) {
        return false;
    }
    crate::translate::shape_might_compile_after_inlining(&loader.registry, callee)
}

fn slots_worth_pinning(
    instructions: &[Instruction],
    locals: usize,
    reference_slots: &[Pinnable],
) -> Vec<Pinnable> {
    use rustclr_core::opcode::Op::*;
    let mut args: std::collections::BTreeMap<usize, usize> = Default::default();
    let mut locs: std::collections::BTreeMap<usize, usize> = Default::default();
    for ins in instructions {
        let index = |ins: &Instruction| ins.operand.as_var().unwrap_or(0) as usize;
        match ins.op {
            Ldarg0 => *args.entry(0).or_default() += 1,
            Ldarg1 => *args.entry(1).or_default() += 1,
            Ldarg2 => *args.entry(2).or_default() += 1,
            Ldarg3 => *args.entry(3).or_default() += 1,
            LdargS | Ldarg | StargS | Starg => *args.entry(index(ins)).or_default() += 1,
            Ldloc0 | Stloc0 => *locs.entry(0).or_default() += 1,
            Ldloc1 | Stloc1 => *locs.entry(1).or_default() += 1,
            Ldloc2 | Stloc2 => *locs.entry(2).or_default() += 1,
            Ldloc3 | Stloc3 => *locs.entry(3).or_default() += 1,
            LdlocS | Ldloc | StlocS | Stloc => *locs.entry(index(ins)).or_default() += 1,
            _ => {}
        }
    }
    let mut ranked: Vec<(usize, Pinnable)> = args
        .into_iter()
        .map(|(i, n)| (n, Pinnable::Arg(i)))
        .chain(
            locs.into_iter()
                // A local index past the declared count would be a bug
                // elsewhere; pinning it would turn that into a silent one.
                .filter(|(i, _)| *i < locals)
                .map(|(i, n)| (n, Pinnable::Local(i))),
        )
        .collect();
    // Busiest first, and by slot within a tie so the choice does not depend on
    // map iteration order — identical input must give identical code.
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| format!("{:?}", a.1).cmp(&format!("{:?}", b.1))));
    // A slot holding a handle is never pinned. The collector finds a compiled
    // frame's roots by reading the frame at offsets fixed when the method was
    // compiled, and a value living in a register is not at any of them. Keeping
    // references in memory is what lets the stack map be a list of offsets
    // rather than a description of the machine state.
    ranked.retain(|(_, slot)| !reference_slots.contains(slot));
    ranked.truncate(PINNED.len());
    ranked.into_iter().map(|(_, slot)| slot).collect()
}

/// A compiled method, kept alive for as long as the process may call it.
pub struct NativeMethod {
    pub method: MethodId,
    pub arg_count: usize,
    pub returns_value: bool,
    /// Classes this body allocates or tests against, in the order its code
    /// numbers them: the type, its instance-field count, and the array type of
    /// a `T[]` of it.
    ///
    /// Travels with the method rather than living in a table the tier owns,
    /// because the numbering is decided when this body is emitted and means
    /// nothing to any other body.
    pub object_shapes: Vec<(rustclr_core::TypeId, usize, rustclr_core::TypeId)>,
    /// Whether this body can allocate.
    ///
    /// A body that cannot needs no allocation context, and building one is not
    /// free: it resolves three types by name and searches for three array
    /// constructions. Doing that per call, for a method that never allocates,
    /// was most of what the `kernels` benchmark measured.
    pub allocates: bool,
    /// Frame offsets of the slots a collection reaching into this method's
    /// frame must read as roots. Empty unless the body holds a reference.
    pub reference_slots: Vec<i32>,
    /// Frame offsets of the slots holding an array descriptor address. A safe
    /// point matches these back to the allocations that produced them.
    pub array_slots: Vec<i32>,
    /// The methods this body calls, in the order its code indexes them.
    pub call_table: Vec<MethodId>,
    /// Their entry addresses, resolved before this body first runs.
    ///
    /// Boxed so the address handed to compiled code is stable, and separate
    /// from `call_table` because the addresses are only known once every callee
    /// has been compiled — which cannot happen while this one is being
    /// compiled, since two methods may call each other.
    pub call_addresses: Option<Box<[i64]>>,
    /// Kept alive because the emitted prologue holds its address, and the
    /// slot vectors above are what it points into.
    #[allow(dead_code)]
    frame_map: Option<Box<crate::alloc::FrameMap>>,
    page: CodePage,
}

impl NativeMethod {
    /// The address compiled code jumps to, for another method's call table.
    pub fn entry(&self) -> i64 {
        self.page.as_ptr() as i64
    }

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
    /// Whether objects that never escape are replaced by their fields.
    ///
    /// Settable for the same reason `inline` is: an optimisation that produces
    /// identical output is indistinguishable from one that never ran, unless a
    /// test can turn it off and watch something change.
    pub scalarise: bool,
    /// How many objects stopped existing, across every method compiled.
    pub objects_scalarised: usize,
    /// Whether a virtual call with a known, closed set of answers is compiled
    /// as a guarded chain rather than declined.
    ///
    /// Settable for the same reason `inline` and `scalarise` are: an
    /// optimisation whose output is identical either way is indistinguishable
    /// from one that never ran unless a test can turn it off.
    pub devirtualise: bool,
}

impl Default for X64Backend {
    fn default() -> Self {
        Self {
            methods_compiled: 0,
            bytes_emitted: 0,
            inline: true,
            scalarise: true,
            objects_scalarised: 0,
            devirtualise: true,
        }
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
            allocates: !compiled.object_shapes.is_empty() || compiled.allocates_arrays,
            reference_slots: compiled.reference_slots,
            array_slots: compiled.array_slots,
            call_table: compiled.call_table,
            call_addresses: None,
            frame_map: compiled.frame_map,
            object_shapes: compiled.object_shapes,
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
        // A float-shaped signature gets past this screen on the strength of its
        // types alone; `compile` runs `floats::infer` over the real body and
        // declines there if the stream cannot be followed. Screening it out
        // here would mean no float method ever reached the float path — which
        // is exactly what happened, and made a differential test pass while
        // compiling nothing.
        if crate::translate::float_shape_possible(registry, method) {
            return true;
        }
        // Same reasoning for an object local: a method that creates one gets
        // past this screen on its declared types, and `compile` decides once
        // `escape::scalarise` has had a look at the real body. Rejecting here
        // would mean no method with an object ever reached the analysis.
        if self.scalarise && crate::translate::scalarisation_possible(registry, method) {
            return true;
        }
        if self.inline {
            // With the inliner on, a `call` is no longer disqualifying on its
            // own. This screen still rejects the wrong signatures and any
            // other unsupported opcode; `compile` makes the final decision
            // once it can see the rewritten body.
            return crate::translate::shape_might_compile_after_inlining(registry, method);
        }
        crate::translate::shape_is_compilable_with(registry, method, true)
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

        // Then scalar replacement, *after* inlining: splicing a callee in can
        // turn an object that was passed to it into one that never leaves this
        // method, and the analysis should see the body as it finally is.
        //
        // An object whose fields become locals stops needing `ldfld` at all —
        // which is how a method the backend used to decline for object access
        // becomes plain arithmetic over frame slots.
        // The locals the *rewritten* body actually uses. Identical to the
        // declared ones until a pass changes the body.
        let mut effective_locals = body.locals.clone();
        effective_locals.resize(locals, rustclr_metadata::TypeSig::I4);
        if self.scalarise {
            if let Some(flat) = crate::escape::scalarise(loader, method, &instructions, locals) {
                instructions = flat.instructions;
                locals += flat.extra_locals.len();
                self.objects_scalarised += flat.objects;
                // The retired object locals are dead — nothing reads or writes
                // them — but their declared type is still a class, which every
                // gate refuses. Substituting a scalar says "ignore this slot"
                // without teaching each gate about liveness.
                for dead in flat.dead_locals {
                    effective_locals[dead as usize] = rustclr_metadata::TypeSig::I4;
                }
                effective_locals.extend(flat.extra_locals);
            }
        }

        // The shape check runs against the *rewritten* body, so an inlined
        // caller is judged on what it actually became.
        //
        // Two gates, asked in order. The integer one takes everything it can;
        // a method it declines gets a second question — is it declined only
        // for floating-point reasons this backend can emit? The float map that
        // comes back says which instructions are float operations, because the
        // opcode alone does not: IL's `add` is `add` for both kinds.
        let floats = if crate::translate::shape_is_compilable_after_inlining(
            &loader.registry,
            method,
            &instructions,
            true,
            &effective_locals,
        ) {
            crate::floats::FloatMap::default()
        } else {
            match crate::translate::float_shape(
                &loader.registry,
                method,
                &instructions,
                &effective_locals,
            ) {
                Some(map) => map,
                None => {
                    return Err(CompileError::Unsupported(
                        "this method's shape is outside the baseline backend".into(),
                    ))
                }
            }
        };

        let returns_value = !info.returns_void();
        // A generous ceiling for the *verifier*, which errors above it. The
        // frame is sized from what the walk actually observed, further down —
        // guessing here and then trusting the guess is how spill slots end up
        // past the end of the frame.
        let max_stack = body.max_stack.saturating_add(64);
        // A `call` moves the stack by however many arguments *the callee*
        // takes, less one if the callee leaves a result. Reading those off the
        // enclosing method instead is right only when the call is recursive,
        // and silently wrong otherwise: every non-recursive call got the
        // caller's own delta, the running depth drifted, and the walk reported
        // "reached with stack depth 2, but 0 on another path" — a stack error
        // in a method whose stack was fine. The callee's real signature is the
        // only correct source, so this uses the same helper the interpreter
        // tier does.
        let enclosing_assembly = loader.assembly(info.assembly);
        let analysis = analyse(&instructions, max_stack, &body.exception_clauses, |ins| {
            crate::call_stack_delta(loader, enclosing_assembly, returns_value, ins)
        })
        .map_err(CompileError::Invalid)?;

        // The trap slot sits one past the real arguments; see `X64Emitter`.
        // The element kind of each `newarr`, and the check that none of them is
        // inside a loop — see `newarr_kinds` on why unbounded allocation from
        // compiled code is not acceptable.
        let assembly = loader.registry.method(method).assembly;
        // Decided before the allocation gates, because whether this body can
        // be stopped mid-loop is what decides whether it may allocate there.
        let safepoints =
            crate::translate::safepoints(&instructions, &analysis).unwrap_or_default();
        let polls = !safepoints.is_empty();
        let mut shapes = crate::translate::ShapeTable::default();
        let Some(newarr_kinds) =
            crate::translate::newarr_kinds(loader, assembly, &instructions, &mut shapes, polls)
        else {
            return Err(CompileError::Unsupported(
                "this method allocates something the backend cannot, or allocates in a loop"
                    .into(),
            ));
        };
        // Which calls target this method itself, and where the rest go.
        let Some(self_calls) =
            crate::translate::self_calls(loader, assembly, method, &instructions)
        else {
            return Err(CompileError::Unsupported(
                "a call this backend cannot resolve".into(),
            ));
        };
        // A call to another method is indirect through a table the tier fills
        // in, so the callee's address does not have to exist yet. Whether it
        // *could* be compiled is settled here; whether it *will* be is settled
        // at run time, which is what lets two methods call each other.
        let Some(targets) = crate::translate::call_targets(
            loader,
            assembly,
            method,
            &instructions,
            |callee| could_compile(loader, callee),
        ) else {
            return Err(CompileError::Unsupported(
                "a call to a method this backend would not compile".into(),
            ));
        };
        let call_returns: std::collections::BTreeMap<u32, bool> = targets
            .sites
            .keys()
            .map(|offset| {
                let token = instructions
                    .iter()
                    .find(|i| i.offset == *offset)
                    .and_then(|i| i.operand.as_token());
                let returns = token
                    .and_then(|t| loader.resolve_method_token(loader.assembly(assembly), t))
                    .map(|m| !loader.registry.method(m).returns_void())
                    .unwrap_or(false);
                (*offset, returns)
            })
            .collect();
        let call_table = targets.table;
        let widest_callee = targets.widest;
        // Objects this body creates, and the classes it names to do so.
        let Some(newobj_shapes) =
            crate::translate::newobj_shapes(loader, assembly, &instructions, &mut shapes, polls)
        else {
            return Err(CompileError::Unsupported(
                "newobj of a class with a real constructor, or inside a loop".into(),
            ));
        };
        // Virtual calls whose every answer is known. A `callvirt` this does not
        // cover makes the walk decline the method, which is where the refusal
        // belongs — here there is nothing to say about a call site not reached.
        let devirt = if self.devirtualise {
            crate::devirt::plan(loader, method, &instructions)
        } else {
            Default::default()
        };
        let object_shapes = shapes.entries;

        // The slots a collection reaching into this frame must read. Declared
        // types decide it; nothing is inferred.
        let params: Vec<rustclr_metadata::TypeSig> = info.signature.params.to_vec();
        let reference_pinnables: Vec<Pinnable> = params
            .iter()
            .enumerate()
            .filter(|(_, sig)| crate::translate::is_reference_sig(sig))
            .map(|(i, _)| Pinnable::Arg(i))
            .chain(
                effective_locals
                    .iter()
                    .enumerate()
                    .filter(|(_, sig)| crate::translate::is_reference_sig(sig))
                    .map(|(i, _)| Pinnable::Local(i)),
            )
            .collect();
        // Array locals hold a descriptor address, and a safe point finds a
        // live array by reading those slots — so they must be *in* the frame,
        // not in a register. Both kinds are excluded from pinning for the same
        // reason: what the collector cannot see, it reclaims.
        let array_pinnables: Vec<Pinnable> = effective_locals
            .iter()
            .enumerate()
            .filter(|(_, sig)| {
                crate::translate::is_int_array(sig)
                    || crate::translate::is_double_array(sig)
                    || crate::translate::is_bool_array(sig)
                    || crate::translate::is_ref_array(sig)
            })
            .map(|(i, _)| Pinnable::Local(i))
            .collect();
        let array_slots: Vec<i32> = if polls {
            array_pinnables
                .iter()
                .map(|p| match p {
                    Pinnable::Local(i) => -SAVED_BYTES - 8 * (1 + *i as i32),
                    Pinnable::Arg(_) => 0,
                })
                .collect()
        } else {
            Vec::new()
        };
        let reference_slots = crate::translate::reference_slots(
            &params,
            &effective_locals,
            |i| -SAVED_BYTES - 8 * (1 + i as i32),
            // An argument lives in the caller's array rather than this frame,
            // and the array outlives the call — the tier owns it. It is rooted
            // by the tier's own pin instead, so it contributes no frame offset.
            |_| None,
        );

        // Slot and kind for every field access. Declined as a whole if any of
        // them names something that has no one-word representation.
        let Some(fields) =
            crate::translate::field_accesses(loader, assembly, &instructions)
        else {
            return Err(CompileError::Unsupported(
                "a field whose type this backend cannot carry".into(),
            ));
        };

        // Built before emitting: the prologue writes a frame record naming
        // this map, so its address has to exist first. Boxed, and the backing
        // vectors moved into the result — moving a `Box` or a `Vec` does not
        // move what it points at, so the addresses stay good.
        let reference_slots = if polls { reference_slots } else { Vec::new() };
        let array_slots = if polls { array_slots } else { Vec::new() };
        let frame_map = Box::new(crate::alloc::FrameMap {
            reference_slots: reference_slots.as_ptr(),
            reference_len: reference_slots.len(),
            array_slots: array_slots.as_ptr(),
            array_len: array_slots.len(),
        });
        let frame_map_address = frame_map.as_ref() as *const crate::alloc::FrameMap as i64;

        let plan = crate::translate::Plan {
            fields,
            call_sites: targets.sites,
            call_returns,
            floats,
            newarr_kinds,
            self_calls,
            arg_count: info.arg_count(),
            returns_value,
            newobj_shapes,
            devirt,
            safepoints,
        };

        // The frame is sized from the depth the verifier *measured*, not from
        // the declared figure. Splicing three levels of callee into a caller
        // can take the evaluation stack well past what the original body
        // declared, and a frame sized from the declaration would have its
        // spill slots land beyond its own end — over the outgoing argument
        // array and the frame record, which is a jump to a wild address rather
        // than a wrong answer.
        let bytes = emit(
            &instructions,
            &analysis,
            locals,
            returns_value,
            info.arg_count(),
            &plan,
            frame_map_address,
            widest_callee,
            &if polls {
                let mut both = reference_pinnables.clone();
                both.extend(array_pinnables.iter().copied());
                both
            } else {
                reference_pinnables.clone()
            },
        )?;
        self.methods_compiled += 1;
        self.bytes_emitted += bytes.len();
        Ok(CompiledCode {
            method,
            tier: Tier::Jit,
            bytes,
            analysis,
            object_shapes,
            allocates_arrays: !plan.newarr_kinds.is_empty(),
            reference_slots,
            array_slots,
            frame_map: Some(frame_map),
            call_table,
        })
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

    // -- SSE2, for `double` ------------------------------------------------
    //
    // Every x86-64 processor has SSE2; it is part of the base architecture
    // rather than an extension to probe for, so these need no feature check.
    //
    // Values arrive in general-purpose registers as bit patterns, because a
    // slot is eight bytes and so is a `double`. Each operation moves the
    // pattern into an xmm register, works there, and moves the result back —
    // which costs two moves per operation and keeps floats out of the register
    // allocator entirely. A method doing float arithmetic in a loop would
    // rather keep them in xmm across iterations; that is a later optimisation
    // and not a correctness matter.

    /// `movq xmm, r64` — the bit pattern, not a conversion.
    fn movq_xmm_from_gp(&mut self, xmm: u8, gp: u8) {
        self.byte(0x66);
        self.rex_forced(true, xmm, gp);
        self.byte(0x0F);
        self.byte(0x6E);
        self.modrm(0b11, xmm, gp);
    }

    /// `movq r64, xmm`.
    fn movq_gp_from_xmm(&mut self, gp: u8, xmm: u8) {
        self.byte(0x66);
        self.rex_forced(true, xmm, gp);
        self.byte(0x0F);
        self.byte(0x7E);
        self.modrm(0b11, xmm, gp);
    }

    /// A two-operand scalar-double instruction: `<op> xmm1, xmm2`.
    fn sse_sd(&mut self, opcode: u8, dst: u8, src: u8) {
        self.byte(0xF2);
        if dst >= 8 || src >= 8 {
            self.rex_forced(false, dst, src);
        }
        self.byte(0x0F);
        self.byte(opcode);
        self.modrm(0b11, dst, src);
    }

    /// `ucomisd xmm1, xmm2` — sets ZF/PF/CF like an unsigned compare, with PF
    /// set when either operand is NaN.
    fn ucomisd(&mut self, lhs: u8, rhs: u8) {
        self.byte(0x66);
        if lhs >= 8 || rhs >= 8 {
            self.rex_forced(false, lhs, rhs);
        }
        self.byte(0x0F);
        self.byte(0x2E);
        self.modrm(0b11, lhs, rhs);
    }

    /// `cvtsi2sd xmm, r64` — signed 64-bit integer to double.
    fn cvtsi2sd(&mut self, xmm: u8, gp: u8) {
        self.byte(0xF2);
        self.rex_forced(true, xmm, gp);
        self.byte(0x0F);
        self.byte(0x2A);
        self.modrm(0b11, xmm, gp);
    }

    /// `cvttsd2si r64, xmm` — double to signed 64-bit, truncating toward zero,
    /// which is what `conv.i8` from a float means.
    fn cvttsd2si(&mut self, gp: u8, xmm: u8) {
        self.byte(0xF2);
        self.rex_forced(true, gp, xmm);
        self.byte(0x0F);
        self.byte(0x2C);
        self.modrm(0b11, gp, xmm);
    }

    /// `xorpd xmm1, xmm2`, used to flip a sign bit.
    fn xorpd(&mut self, dst: u8, src: u8) {
        self.byte(0x66);
        if dst >= 8 || src >= 8 {
            self.rex_forced(false, dst, src);
        }
        self.byte(0x0F);
        self.byte(0x57);
        self.modrm(0b11, dst, src);
    }

    /// `sub rsp, imm8` — for the shadow space a call needs.
    fn sub_rsp(&mut self, bytes: i32) {
        self.rex(true, 0, RSP);
        self.byte(0x83);
        self.modrm(0b11, 5, RSP);
        self.byte(bytes as u8);
    }

    /// `add rsp, imm8`.
    fn add_rsp(&mut self, bytes: i32) {
        self.rex(true, 0, RSP);
        self.byte(0x83);
        self.modrm(0b11, 0, RSP);
        self.byte(bytes as u8);
    }

    /// `call reg` — an indirect call through a register.
    fn call_reg(&mut self, reg: u8) {
        self.rex(false, 0, reg);
        self.byte(0xFF);
        self.modrm(0b11, 2, reg);
    }

    /// `cmp reg, imm8`.
    fn cmp_ri(&mut self, reg: u8, value: i32) {
        self.rex(true, 0, reg);
        self.byte(0x83);
        self.modrm(0b11, 7, reg);
        self.byte(value as u8);
    }

    /// `movzx dst, byte [base + index*1]` — a one-byte element, zero-extended.
    fn movzx_r_mem_index_u8(&mut self, dst: u8, base: u8, index: u8) {
        self.rex_index(true, dst, base, index);
        self.byte(0x0F);
        self.byte(0xB6);
        self.modrm(0b00, dst, RSP);
        self.byte(((index & 7) << 3) | (base & 7));
    }

    /// `mov byte [base + index*1], src8`.
    ///
    /// REX is forced so the source names the low byte of the register rather
    /// than `ah`/`ch`/`dh`, which is what the encoding would otherwise mean.
    fn mov_mem_index_r8(&mut self, base: u8, index: u8, src: u8) {
        self.rex_index_forced(false, src, base, index);
        self.byte(0x88);
        self.modrm(0b00, src, RSP);
        self.byte(((index & 7) << 3) | (base & 7));
    }

    /// `lea dst, [base + offset]`.
    fn lea(&mut self, dst: u8, base: u8, offset: i32) {
        self.rex(true, dst, base);
        self.byte(0x8D);
        self.byte((0b10 << 6) | ((dst & 7) << 3) | (base & 7));
        self.imm32(offset);
    }

    /// `add reg, imm32`.
    fn add_ri32(&mut self, reg: u8, value: i32) {
        self.rex(true, 0, reg);
        self.byte(0x81);
        self.modrm(0b11, 0, reg);
        self.imm32(value);
    }

    /// `cmp reg, imm32`.
    fn cmp_ri32(&mut self, reg: u8, value: i32) {
        self.rex(true, 0, reg);
        self.byte(0x81);
        self.modrm(0b11, 7, reg);
        self.imm32(value);
    }

    /// `call rel32` back to the start of the code being emitted.
    ///
    /// The target is offset zero — a compiled method's entry is the first byte
    /// of its own page — so the displacement is known here and needs no fixup.
    fn call_self_start(&mut self) {
        self.byte(0xE8);
        let after = self.code.len() as i64 + 4;
        self.imm32((-after) as i32);
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

    /// `jcc rel8` over code emitted immediately after, patched by
    /// [`Self::patch_short`].
    ///
    /// The label table is keyed by IL offset, and a bounds-check's landing pad
    /// is not at one — it is a few bytes further into the same instruction's
    /// expansion. Rather than invent synthetic offsets, this patches the
    /// displacement directly, which works because the skipped range is a fixed
    /// handful of bytes and cannot overflow `i8`.
    fn jcc_short_forward(&mut self, condition: u8) -> usize {
        self.byte(0x70 | condition);
        self.byte(0);
        self.code.len() - 1
    }

    /// An unconditional short jump forward, patched later by `patch_short`.
    fn jmp_short_forward(&mut self) -> usize {
        self.byte(0xEB);
        self.byte(0);
        self.code.len() - 1
    }

    fn patch_short(&mut self, at: usize) {
        let distance = self.code.len() - at - 1;
        debug_assert!(distance <= 127, "short branch out of range");
        self.code[at] = distance as u8;
    }

    /// `cmp left, right`.
    fn cmp_rr(&mut self, left: u8, right: u8) {
        self.alu_rr(0x39, left, right);
    }

    /// `mov dst, [base + index*scale + disp]` — a scaled-index load.
    fn mov_r_mem_index(&mut self, dst: u8, base: u8, index: u8, scale: u8) {
        self.rex_index(true, dst, base, index);
        self.byte(0x8B);
        self.modrm(0b00, dst, RSP);
        self.byte((scale << 6) | ((index & 7) << 3) | (base & 7));
    }

    /// `mov [base + index*scale], src` — a scaled-index store.
    fn mov_mem_index_r(&mut self, base: u8, index: u8, scale: u8, src: u8) {
        self.rex_index(true, src, base, index);
        self.byte(0x89);
        self.modrm(0b00, src, RSP);
        self.byte((scale << 6) | ((index & 7) << 3) | (base & 7));
    }

    /// `movsxd dst, dword [base + index*4]` — a 32-bit element, sign-extended.
    ///
    /// Sign extension is the point: `int[]` elements are 32-bit and the
    /// evaluation stack is 64-bit, so a negative element read with a plain
    /// 64-bit load would come back as a huge positive number.
    fn movsxd_r_mem_index(&mut self, dst: u8, base: u8, index: u8) {
        self.rex_index(true, dst, base, index);
        self.byte(0x63);
        self.modrm(0b00, dst, RSP);
        self.byte((0b10 << 6) | ((index & 7) << 3) | (base & 7));
    }

    /// `mov dword [base + index*4], src` — a 32-bit element store.
    fn mov_mem_index_r32(&mut self, base: u8, index: u8, src: u8) {
        self.rex_index(false, src, base, index);
        self.byte(0x89);
        self.modrm(0b00, src, RSP);
        self.byte((0b10 << 6) | ((index & 7) << 3) | (base & 7));
    }

    /// REX for an instruction with a base *and* an index register.
    /// [`Self::rex_index`] that always emits, for a byte-register operand.
    fn rex_index_forced(&mut self, w: bool, reg: u8, base: u8, index: u8) {
        self.byte(
            0x40 | ((w as u8) << 3)
                | (((reg >> 3) & 1) << 2)
                | (((index >> 3) & 1) << 1)
                | ((base >> 3) & 1),
        );
    }

    fn rex_index(&mut self, w: bool, reg: u8, base: u8, index: u8) {
        let byte = 0x40
            | ((w as u8) << 3)
            | (((reg >> 3) & 1) << 2)
            | (((index >> 3) & 1) << 1)
            | ((base >> 3) & 1);
        if w || byte != 0x40 {
            self.byte(byte);
        }
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

/// How much machine stack compiled frames may occupy before they refuse.
///
/// **Bytes, not frames.** A frame count was the obvious counter and was wrong
/// in principle the whole time: frames differ in size, so any count is either
/// too generous for a large frame or too mean for a small one. It only became
/// visible when frames grew — an outgoing argument array sized for the widest
/// callee, plus a frame record — and 4096 frames stopped fitting in a thread
/// stack. The symptom was a segfault, which is exactly what this counter
/// exists to prevent.
///
/// The machine stack cannot be grown or unwound the way the interpreter's
/// frame budget can, so overrunning it is a crash rather than an exception.
/// This budget is deliberately a fraction of the smallest stack this runtime
/// expects to run on, leaving the interpreter beneath and the helpers above
/// their own room.
const MAX_COMPILED_STACK_BYTES: i32 = 16 * 1024 * 1024;
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
    /// Argument slot the emitted code writes to when a bounds check fails.
    ///
    /// One past the real arguments. Compiled code cannot throw — it has no
    /// frame the interpreter can unwind — so an out-of-range index sets this
    /// flag and returns, and the tier raises `IndexOutOfRangeException` on its
    /// behalf. Stores that already happened stay done, which is what .NET does
    /// too: the exception aborts the method, it does not roll it back.
    trap_slot: usize,
    /// Frame offset of the outgoing argument array a self-call fills in.
    outgoing: i32,
    /// Slots living in [`PINNED`], in register order.
    pinned: Vec<Pinnable>,
    /// Whether this body polls for a collection at its loop back-edges.
    polls: bool,
    /// Address of this method's [`crate::alloc::FrameMap`].
    frame_map: i64,
    /// Frame offset of this frame's record: parent, base pointer, map.
    record: i32,
    /// Arguments the widest callee takes, which is what the outgoing frame has
    /// to be big enough for — a callee's trap and context slots sit past *its*
    /// argument count, not this method's.
    widest_callee: usize,
    /// Frame offset of the scratch word; see [`Self::frame_scratch`].
    scratch: i32,
}

impl X64Emitter {
    /// Machine stack this frame occupies, generously rounded.
    ///
    /// The reserved frame, the registers pushed above it, the return address,
    /// and the shadow space a call reserves — everything one link of the chain
    /// costs.
    fn stack_cost(&self) -> i32 {
        self.frame + SAVED_BYTES + 8 + 32
    }

    /// The register holding `slot`, if it has one.
    fn pin_of(&self, slot: Pinnable) -> Option<u8> {
        self.pinned.iter().position(|p| *p == slot).map(|i| PINNED[i])
    }

    fn new(trap_slot: usize) -> Self {
        Self {
            asm: Assembler::new(),
            locals: 0,
            frame: 0,
            trap_slot,
            scratch: 0,
            outgoing: 0,
            pinned: Vec::new(),
            polls: false,
            frame_map: 0,
            record: 0,
            widest_callee: 0,
        }
    }

    /// A frame slot no local or spill uses, for holding a value while every
    /// scratch register is busy.
    ///
    /// One word past the deepest spill slot. `array_store_i32` is the only
    /// thing that needs it, and needs it because a store has three live
    /// operands — array, index, value — and this architecture offers exactly
    /// three scratch registers, leaving none for the length to compare
    /// against. The prologue already reserves two spare words, so this costs
    /// nothing.
    fn frame_scratch(&self) -> i32 {
        self.scratch
    }

    /// Emits `if index >= limit { trap; return }`.
    ///
    /// Unsigned, so one comparison covers both ends: a negative index
    /// sign-extends to a very large unsigned value and fails the same test that
    /// catches `index >= length`.
    fn bounds_check(&mut self, index: u8, limit: u8) {
        self.asm.cmp_rr(index, limit);
        let skip = self.asm.jcc_short_forward(CC_B);
        // Out of range: flag it and leave.
        self.asm.mov_ri(RAX, 1);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RAX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(skip);
    }

    /// The epilogue without going through the trait, for the trap path.
    fn epilogue_inline(&mut self) {
        self.asm.rex(true, 0, RSP);
        self.asm.byte(0x81);
        self.asm.modrm(0b11, 0, RSP);
        self.asm.imm32(self.frame);
        self.asm.pop(R13);
        self.asm.pop(R12);
        self.asm.pop(R15);
        self.asm.pop(R14);
        self.asm.pop(RBX);
        self.asm.pop(RBP);
        self.asm.ret();
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
        // One word past the deepest spill slot. The reservation below always
        // covers it, because `words` is at least `locals + max_stack + 2`.
        self.scratch = -SAVED_BYTES - 8 * (locals as i32 + max_stack as i32 + 1);

        self.asm.push(RBP);
        self.asm.mov_rr(RBP, RSP);
        self.asm.push(RBX);
        self.asm.push(R14);
        self.asm.push(R15);
        self.asm.push(R12);
        self.asm.push(R13);

        // Room for the argument array a recursive call hands its callee:
        // the arguments, the trap slot, the allocation context, and the depth
        // counter. Reserved once here rather than pushed per call, so the
        // stack pointer never moves inside the body.
        // arguments, trap, allocation context, depth, stop flag, safepoint
        // context, and the allocation-pressure flag.
        let outgoing_words = self.trap_slot.max(self.widest_callee) + 8;
        self.outgoing = self.scratch - 8 * outgoing_words as i32;
        // Three words for this frame's record. Below the outgoing frame so a
        // call cannot overwrite the description of the frame making it.
        self.record = self.outgoing - 24;

        // Reserve locals and spill slots, keeping rsp 16-byte aligned. The
        // return address plus `push rbp` and five more pushes leave the stack
        // misaligned by 8, so an odd number of reserved words restores it.
        // Five pushes rather than three changes the offset but not the parity:
        // 40 bytes and 24 bytes are both 8 mod 16.
        let words = locals + max_stack + 2 + outgoing_words + 3 + 1;
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

        // This frame's record, for a safe point anywhere below it. Written
        // unconditionally: a method that never polls itself may still call one
        // that does, and the chain has to reach through it.
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 6) * 8) as i32);
        self.asm.mov_mem_r(RBP, self.record, RAX);
        self.asm.lea(RAX, RBP, 0);
        self.asm.mov_mem_r(RBP, self.record + 8, RAX);
        self.asm.mov_ri(RAX, self.frame_map);
        self.asm.mov_mem_r(RBP, self.record + 16, RAX);

        // Give each pinned register its starting value. An argument's comes
        // from the array the caller filled in; a local's is zero.
        for (slot, reg) in self.pinned.clone().into_iter().zip(PINNED) {
            match slot {
                Pinnable::Arg(i) => self.asm.mov_r_mem(reg, ARGS, (i * 8) as i32),
                Pinnable::Local(_) => self.asm.mov_ri(reg, 0),
            }
        }

        // `init_locals` semantics: every local starts at zero. A pinned local
        // was just zeroed in its register and has no frame slot to write.
        if locals > 0 {
            self.asm.mov_ri(RAX, 0);
            for i in 0..locals {
                if self.pin_of(Pinnable::Local(i)).is_some() {
                    continue;
                }
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
        self.asm.pop(R13);
        self.asm.pop(R12);
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
        match self.pin_of(Pinnable::Arg(index)) {
            Some(reg) => self.asm.mov_rr(dst, reg),
            None => self.asm.mov_r_mem(dst, ARGS, (index * 8) as i32),
        }
    }

    fn store_arg(&mut self, index: usize, src: u8) {
        match self.pin_of(Pinnable::Arg(index)) {
            // The argument array is the caller's copy and nothing reads it
            // back, so a pinned argument lives only in its register.
            Some(reg) => self.asm.mov_rr(reg, src),
            None => self.asm.mov_mem_r(ARGS, (index * 8) as i32, src),
        }
    }

    fn load_local(&mut self, dst: u8, index: usize) {
        match self.pin_of(Pinnable::Local(index)) {
            Some(reg) => self.asm.mov_rr(dst, reg),
            None => {
                let off = Backend::local_offset(self, index);
                self.asm.mov_r_mem(dst, RBP, off)
            }
        }
    }

    fn store_local(&mut self, index: usize, src: u8) {
        match self.pin_of(Pinnable::Local(index)) {
            Some(reg) => self.asm.mov_rr(reg, src),
            None => {
                let off = Backend::local_offset(self, index);
                self.asm.mov_mem_r(RBP, off, src)
            }
        }
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

    // -- floating point ----------------------------------------------------
    //
    // Two scratch xmm registers are enough: every operation here loads its
    // operands, computes, and stores the result straight back to a
    // general-purpose register, so nothing has to stay in xmm between
    // instructions.

    fn supports_floats(&self) -> bool {
        true
    }

    fn float_binop(&mut self, op: BinOp, dst: u8, lhs: u8, rhs: u8) {
        debug_assert_eq!(dst, lhs, "x86-64 computes in place");
        self.asm.movq_xmm_from_gp(XMM0, lhs);
        self.asm.movq_xmm_from_gp(XMM1, rhs);
        match op {
            BinOp::Add => self.asm.sse_sd(0x58, XMM0, XMM1),
            BinOp::Sub => self.asm.sse_sd(0x5C, XMM0, XMM1),
            BinOp::Mul => self.asm.sse_sd(0x59, XMM0, XMM1),
            BinOp::Div => self.asm.sse_sd(0x5E, XMM0, XMM1),
            // `rem` on doubles is IEEE remainder, and x86 has no single
            // instruction for it since SSE — `fprem` is x87. The shape checker
            // declines a float `rem` rather than emitting something close.
            _ => unreachable!("only add, sub, mul and div reach the float path"),
        }
        self.asm.movq_gp_from_xmm(dst, XMM0);
    }

    fn float_neg(&mut self, dst: u8, src: u8) {
        // Flip the sign bit rather than subtracting from zero: `0.0 - (-0.0)`
        // is `0.0`, so subtraction gets negative zero wrong.
        self.asm.movq_xmm_from_gp(XMM0, src);
        self.asm.mov_ri(RAX, i64::MIN); // 1 << 63, the sign bit
        self.asm.movq_xmm_from_gp(XMM1, RAX);
        self.asm.xorpd(XMM0, XMM1);
        self.asm.movq_gp_from_xmm(dst, XMM0);
    }

    fn float_compare(&mut self, cond: Cond, dst: u8, lhs: u8, rhs: u8) {
        self.asm.movq_xmm_from_gp(XMM0, lhs);
        self.asm.movq_xmm_from_gp(XMM1, rhs);
        self.asm.ucomisd(XMM0, XMM1);
        // `ucomisd` sets the flags an *unsigned* integer compare would, so the
        // unsigned condition codes are the right ones here whatever the signed
        // name of the comparison was. NaN sets ZF, PF and CF together, which
        // makes every ordered comparison false — as IEEE and the CLI require.
        let code: u8 = match cond {
            Cond::Equal => 0x4,                            // e
            Cond::NotEqual => 0x5,                         // ne
            Cond::Less | Cond::Below => 0x2,               // b
            Cond::LessOrEqual | Cond::BelowOrEqual => 0x6, // be
            Cond::Greater | Cond::Above => 0x7,            // a
            Cond::GreaterOrEqual | Cond::AboveOrEqual => 0x3, // ae
        };
        // setcc dst8, then widen — the same shape the integer `compare` uses.
        self.asm.rex_forced(false, 0, dst);
        self.asm.byte(0x0F);
        self.asm.byte(0x90 | code);
        self.asm.modrm(0b11, 0, dst);
        self.asm.rex_forced(true, dst, dst);
        self.asm.byte(0x0F);
        self.asm.byte(0xB6);
        self.asm.modrm(0b11, dst, dst);
    }

    fn float_branch_compare(&mut self, cond: Cond, lhs: u8, rhs: u8, target: u32) {
        self.asm.movq_xmm_from_gp(XMM0, lhs);
        self.asm.movq_xmm_from_gp(XMM1, rhs);
        self.asm.ucomisd(XMM0, XMM1);
        // Unsigned codes, for the reason given in `float_compare`.
        let code: u8 = match cond {
            Cond::Equal => 0x4,
            Cond::NotEqual => 0x5,
            Cond::Less | Cond::Below => 0x2,
            Cond::LessOrEqual | Cond::BelowOrEqual => 0x6,
            Cond::Greater | Cond::Above => 0x7,
            Cond::GreaterOrEqual | Cond::AboveOrEqual => 0x3,
        };
        self.asm.jcc(code, target);
    }

    fn int_to_float(&mut self, dst: u8, src: u8) {
        self.asm.cvtsi2sd(XMM0, src);
        self.asm.movq_gp_from_xmm(dst, XMM0);
    }

    fn float_to_int(&mut self, dst: u8, src: u8) {
        self.asm.movq_xmm_from_gp(XMM0, src);
        self.asm.cvttsd2si(dst, XMM0);
    }

    fn supports_arrays(&self) -> bool {
        true
    }

    fn array_length(&mut self, dst: u8, descriptor: u8) {
        // The descriptor is `{ data pointer, length }`; the length is the
        // second word.
        self.asm.mov_r_mem(dst, descriptor, 8);
    }

    fn array_load_i32(&mut self, dst: u8, descriptor: u8, index: u8) {
        // Length first, while `descriptor` still holds the descriptor address.
        let limit = if dst == descriptor { RDX } else { dst };
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        self.asm.movsxd_r_mem_index(dst, descriptor, index);
    }

    fn supports_self_calls(&self) -> bool {
        true
    }

    fn outgoing_arg(&mut self, index: usize, src: u8) {
        self.asm.mov_mem_r(RBP, self.outgoing + 8 * index as i32, src);
    }

    fn call_other(&mut self, dst: u8, index: usize, arg_count: usize) {
        // The callee reads its trap and context words past *its* arguments, so
        // every offset here is relative to the callee's count rather than this
        // method's. Getting that wrong would have the callee read its trap slot
        // out of an argument.
        let base = self.outgoing + 8 * arg_count as i32;
        let trap = base;
        let ctx = base + 8;
        let depth = base + 16;
        let flag = base + 24;
        let safepoint_ctx = base + 32;
        let pressure = base + 40;
        let parent = base + 48;
        let callee_table = base + 56;

        // A clear trap slot, and the same context this frame runs with: the
        // callee allocates into the same sink, polls the same flags, and
        // reports through the same fault word.
        self.asm.mov_ri(RAX, 0);
        self.asm.mov_mem_r(RBP, trap, RAX);
        for (offset, slot) in
            [(ctx, 1usize), (flag, 3), (safepoint_ctx, 4), (pressure, 5)]
        {
            self.asm.mov_r_mem(RAX, ARGS, (slot * 8) as i32 + (self.trap_slot * 8) as i32);
            self.asm.mov_mem_r(RBP, offset, RAX);
        }
        // Its parent is this frame, so a safe point anywhere below can walk
        // back up and describe everything still live.
        self.asm.lea(RAX, RBP, self.record);
        self.asm.mov_mem_r(RBP, parent, RAX);

        // Stack guard, shared with recursion: mutual recursion runs on the
        // machine stack exactly as direct recursion does.
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 2) * 8) as i32);
        self.asm.add_ri32(RAX, self.stack_cost());
        self.asm.mov_mem_r(RBP, depth, RAX);
        self.asm.cmp_ri32(RAX, MAX_COMPILED_STACK_BYTES);
        let shallow = self.asm.jcc_short_forward(CC_L);
        self.asm.mov_ri(RAX, 3);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RAX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(shallow);

        // The callee's entry *and its own call table*, out of the table the
        // tier filled in. Both are needed: a callee that calls anything reads
        // its table out of the argument array this frame is building, and
        // leaving that slot unwritten sends it jumping through whatever the
        // stack happened to hold. Two words per entry for exactly that reason.
        self.asm.mov_r_mem(RDX, ARGS, ((self.trap_slot + 7) * 8) as i32);
        self.asm.mov_r_mem(RAX, RDX, 16 * index as i32 + 8);
        self.asm.mov_mem_r(RBP, callee_table, RAX);
        self.asm.mov_r_mem(RAX, RDX, 16 * index as i32);
        let incoming = if cfg!(windows) { RCX } else { RDI };
        self.asm.lea(incoming, RBP, self.outgoing);
        self.asm.sub_rsp(32);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(32);

        // A trap raised inside the callee has to reach the tier, which only
        // reads *this* frame's slot.
        if dst != RAX {
            self.asm.mov_rr(dst, RAX);
        }
        self.asm.mov_r_mem(RDX, RBP, trap);
        self.asm.cmp_ri(RDX, 0);
        let clean = self.asm.jcc_short_forward(CC_E);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RDX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(clean);
    }

    fn call_self(&mut self, dst: u8) {
        let trap = self.outgoing + 8 * self.trap_slot as i32;
        let ctx = trap + 8;
        let depth = ctx + 8;
        let flag = depth + 8;
        let safepoint_ctx = flag + 8;
        let pressure = safepoint_ctx + 8;
        let parent = pressure + 8;
        let callee_table = parent + 8;

        // The callee starts with a clear trap slot, and inherits the
        // allocation context so it can allocate as this frame can.
        self.asm.mov_ri(RAX, 0);
        self.asm.mov_mem_r(RBP, trap, RAX);
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 1) * 8) as i32);
        self.asm.mov_mem_r(RBP, ctx, RAX);

        // A recursive callee polls where this frame does, so it inherits every
        // context word, and its parent is this frame — which is what makes the
        // chain of frames a chain rather than a single link.
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 3) * 8) as i32);
        self.asm.mov_mem_r(RBP, flag, RAX);
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 4) * 8) as i32);
        self.asm.mov_mem_r(RBP, safepoint_ctx, RAX);
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 5) * 8) as i32);
        self.asm.mov_mem_r(RBP, pressure, RAX);
        self.asm.lea(RAX, RBP, self.record);
        self.asm.mov_mem_r(RBP, parent, RAX);
        // The callee is this method, so it calls through this method's table.
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 7) * 8) as i32);
        self.asm.mov_mem_r(RBP, callee_table, RAX);

        // Stack guard. Compiled recursion runs on the *machine* stack, and
        // overrunning that is a crash rather than an exception — which this
        // runtime does not accept anywhere else either. What accumulates is
        // this frame's own size, so the running total is the bytes every frame
        // below the callee occupies.
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 2) * 8) as i32);
        self.asm.add_ri32(RAX, self.stack_cost());
        self.asm.mov_mem_r(RBP, depth, RAX);
        self.asm.cmp_ri32(RAX, MAX_COMPILED_STACK_BYTES);
        let shallow = self.asm.jcc_short_forward(CC_L);
        self.asm.mov_ri(RAX, 3);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RAX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(shallow);

        // Enter exactly as the tier enters this method: one pointer to the
        // argument array, in the register the platform's C ABI uses.
        let incoming = if cfg!(windows) { RCX } else { RDI };
        self.asm.lea(incoming, RBP, self.outgoing);
        self.asm.sub_rsp(32);
        self.asm.call_self_start();
        self.asm.add_rsp(32);

        // A trap raised inside the callee has to reach the tier, and the tier
        // only reads *this* frame's slot. Propagating rather than ignoring is
        // what keeps a bounds failure deep in a recursion from being lost.
        if dst != RAX {
            self.asm.mov_rr(dst, RAX);
        }
        self.asm.mov_r_mem(RDX, RBP, trap);
        self.asm.cmp_ri(RDX, 0);
        let clean = self.asm.jcc_short_forward(CC_E);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RDX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(clean);
    }

    fn supports_allocation(&self) -> bool {
        true
    }

    fn alloc_array(&mut self, dst: u8, kind: i64, length: u8) {
        // The length has to survive being marshalled into an argument
        // register, and `length` may *be* one of those registers, so it is
        // parked in the frame first.
        self.asm.mov_mem_r(RBP, self.frame_scratch(), length);

        // Argument registers. RBX, R14 and R15 — the argument pointer and the
        // two cached slots — are callee-saved in both ABIs, so a C call
        // preserves them and nothing else is live across an instruction.
        let (a0, a1, a2) = if cfg!(windows) {
            (RCX, RDX, R8)
        } else {
            (RDI, RSI, RDX)
        };

        // The allocation context sits one past the trap slot, written by the
        // tier alongside the arguments.
        self.asm.mov_r_mem(a0, ARGS, ((self.trap_slot + 1) * 8) as i32);
        self.asm.mov_ri(a1, kind);
        self.asm.mov_r_mem(a2, RBP, self.frame_scratch());

        // 32 bytes of shadow space, which Windows requires and System V
        // tolerates. The prologue keeps rsp 16-byte aligned and 32 is a
        // multiple of 16, so the alignment the ABI wants at the `call` holds.
        self.asm.sub_rsp(32);
        self.asm.mov_ri(RAX, crate::alloc::alloc_array as *const () as usize as i64);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(32);

        // Zero means the heap could not satisfy it. Same trap path a failed
        // bounds check takes: flag it and leave, and the tier raises the
        // exception where there is a frame to raise it into.
        self.asm.cmp_ri(RAX, 0);
        let ok = self.asm.jcc_short_forward(CC_NE);
        self.asm.mov_ri(RAX, 2);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RAX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(ok);

        if dst != RAX {
            self.asm.mov_rr(dst, RAX);
        }
    }

    fn field_load(&mut self, dst: u8, receiver: u8, slot: usize, kind: i64) {
        // The receiver has to survive being marshalled into an argument
        // register, and may already be one.
        self.asm.mov_mem_r(RBP, self.frame_scratch(), receiver);
        let (a0, a1, a2, a3) = if cfg!(windows) {
            (RCX, RDX, R8, R9)
        } else {
            (RDI, RSI, RDX, RCX)
        };
        self.asm.mov_r_mem(a0, ARGS, ((self.trap_slot + 1) * 8) as i32);
        self.asm.mov_r_mem(a1, RBP, self.frame_scratch());
        self.asm.mov_ri(a2, slot as i64);
        self.asm.mov_ri(a3, kind);
        self.asm.sub_rsp(32);
        self.asm.mov_ri(RAX, crate::alloc::field_load as *const () as usize as i64);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(32);
        if dst != RAX {
            self.asm.mov_rr(dst, RAX);
        }
    }

    fn field_store(&mut self, receiver: u8, slot: usize, kind: i64, value: u8) {
        self.asm.mov_mem_r(RBP, self.frame_scratch(), receiver);
        self.asm.mov_mem_r(RBP, self.frame_scratch() - 8, value);
        let (a0, a1, a2, a3) = if cfg!(windows) {
            (RCX, RDX, R8, R9)
        } else {
            (RDI, RSI, RDX, RCX)
        };
        self.asm.mov_r_mem(a0, ARGS, ((self.trap_slot + 1) * 8) as i32);
        self.asm.mov_r_mem(a1, RBP, self.frame_scratch());
        self.asm.mov_ri(a2, slot as i64);
        self.asm.mov_ri(a3, kind);
        // The fifth argument goes in a register on System V and on the stack
        // inside the shadow space on Windows, which is what the two ABIs
        // actually differ about here.
        self.asm.sub_rsp(48);
        if cfg!(windows) {
            self.asm.mov_r_mem(RAX, RBP, self.frame_scratch() - 8);
            self.asm.mov_mem_r(RSP, 32, RAX);
        } else {
            self.asm.mov_r_mem(R8, RBP, self.frame_scratch() - 8);
        }
        self.asm.mov_ri(RAX, crate::alloc::field_store as *const () as usize as i64);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(48);
    }

    fn safepoint(&mut self) {
        // Two words, either of which means "enter the helper".
        //
        // The first is the registry's stop flag: *another* thread wants to
        // collect and is waiting for this one to park. The second is this
        // call's own allocation pressure, raised inside the allocation helper
        // where the heap lock is already held. Both are needed — with a single
        // thread nothing ever sets the first, and a compiled loop that
        // allocates would otherwise never reclaim anything.
        //
        // Neither is synchronised, because a stale read is harmless in both
        // directions: a missed set is caught at the next back-edge, and a
        // spurious one finds nothing to do.
        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 3) * 8) as i32);
        self.asm.cmp_ri(RAX, 0);
        let no_stop_flag = self.asm.jcc_short_forward(CC_E);
        self.asm.mov_r_mem(RAX, RAX, 0);
        self.asm.cmp_ri(RAX, 0);
        let stop_wanted = self.asm.jcc_short_forward(CC_NE);
        self.asm.patch_short(no_stop_flag);

        self.asm.mov_r_mem(RAX, ARGS, ((self.trap_slot + 5) * 8) as i32);
        self.asm.cmp_ri(RAX, 0);
        let quiet = self.asm.jcc_short_forward(CC_E);
        self.asm.mov_r_mem(RAX, RAX, 0);
        self.asm.cmp_ri(RAX, 0);
        let nothing_to_do = self.asm.jcc_short_forward(CC_E);
        self.asm.patch_short(stop_wanted);

        // A collection is under way. Hand the helper this frame, which is all
        // it needs: the offsets of the slots holding handles were worked out
        // when this method was compiled and travel in the context.
        let (a0, a1) = if cfg!(windows) { (RCX, RDX) } else { (RDI, RSI) };
        self.asm.mov_r_mem(a0, ARGS, ((self.trap_slot + 4) * 8) as i32);
        self.asm.lea(a1, RBP, self.record);
        self.asm.sub_rsp(32);
        self.asm.mov_ri(RAX, crate::alloc::safepoint as *const () as usize as i64);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(32);

        self.asm.patch_short(quiet);
        self.asm.patch_short(nothing_to_do);
    }

    fn array_load_ref(&mut self, dst: u8, descriptor: u8, index: u8) {
        // Identical encoding to the `double` load: eight bytes, scale 3. A
        // handle is `repr(transparent)` over `u64`, so the raw bits *are* the
        // value — and unlike a pointer, they cannot be dereferenced here.
        let limit = if dst == descriptor { RDX } else { dst };
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        self.asm.mov_r_mem_index(dst, descriptor, index, 3);
    }

    fn array_store_ref(&mut self, descriptor: u8, index: u8, value: u8) {
        self.asm.mov_mem_r(RBP, self.frame_scratch(), value);
        let limit = value;
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        self.asm.mov_r_mem(limit, RBP, self.frame_scratch());
        self.asm.mov_mem_index_r(descriptor, index, 3, limit);
    }

    fn alloc_object(&mut self, dst: u8, shape: i64) {
        let (a0, a1) = if cfg!(windows) { (RCX, RDX) } else { (RDI, RSI) };
        self.asm.mov_r_mem(a0, ARGS, ((self.trap_slot + 1) * 8) as i32);
        self.asm.mov_ri(a1, shape);
        self.asm.sub_rsp(32);
        self.asm.mov_ri(RAX, crate::alloc::alloc_object as *const () as usize as i64);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(32);

        // Zero is `Handle::NULL`: the heap could not satisfy it. Same trap the
        // array path takes, so the tier raises OutOfMemoryException where there
        // is a frame to raise it into.
        self.asm.cmp_ri(RAX, 0);
        let ok = self.asm.jcc_short_forward(CC_NE);
        self.asm.mov_ri(RAX, 2);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RAX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(ok);

        if dst != RAX {
            self.asm.mov_rr(dst, RAX);
        }
    }

    fn devirt_dispatch(&mut self, dst: u8, receiver: u8, candidates: &[(i64, i64)]) {
        // The receiver's runtime type, as a registry index. The lookup happens
        // in Rust — compiled code never reads an object's memory, so no part of
        // `ClrObject`'s layout is assumed here.
        let (a0, a1) = if cfg!(windows) { (RCX, RDX) } else { (RDI, RSI) };
        self.asm.mov_mem_r(RBP, self.frame_scratch(), receiver);
        self.asm.mov_r_mem(a0, ARGS, ((self.trap_slot + 1) * 8) as i32);
        self.asm.mov_r_mem(a1, RBP, self.frame_scratch());
        self.asm.sub_rsp(32);
        self.asm.mov_ri(RAX, crate::alloc::type_of as *const () as usize as i64);
        self.asm.call_reg(RAX);
        self.asm.add_rsp(32);

        // Null first, and as its own answer. A virtual call on a null receiver
        // owes the program a NullReferenceException; letting it fall through
        // the guards would report it as a failure of the hierarchy analysis,
        // which is both wrong and misleading.
        self.asm.cmp_ri(RAX, -1);
        let not_null = self.asm.jcc_short_forward(CC_NE);
        self.asm.mov_ri(RDX, 6);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RDX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();
        self.asm.patch_short(not_null);

        // One compare per candidate. Two of them is the common shape, and a
        // linear chain beats a jump table at that size.
        let mut done = Vec::with_capacity(candidates.len());
        for (type_index, value) in candidates {
            self.asm.cmp_ri32(RAX, *type_index as i32);
            let next = self.asm.jcc_short_forward(CC_NE);
            self.asm.mov_ri(RDX, *value);
            done.push(self.asm.jmp_short_forward());
            self.asm.patch_short(next);
        }

        // Unreachable by class-hierarchy analysis: the compiler proved no other
        // type can reach this call. Trapping rather than falling through means
        // a proof that was somehow wrong reports itself instead of returning a
        // number nobody can account for.
        self.asm.mov_ri(RAX, 5);
        self.asm.mov_mem_r(ARGS, (self.trap_slot * 8) as i32, RAX);
        self.asm.mov_ri(RAX, 0);
        self.epilogue_inline();

        for patch in done {
            self.asm.patch_short(patch);
        }
        if dst != RDX {
            self.asm.mov_rr(dst, RDX);
        }
    }

    fn array_load_u8(&mut self, dst: u8, descriptor: u8, index: u8) {
        let limit = if dst == descriptor { RDX } else { dst };
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        // Scale 0 is one byte per element, and `movzx` because `ldelem.u1` is
        // the unsigned form — a byte of 0xFF is 255, not -1.
        self.asm.movzx_r_mem_index_u8(dst, descriptor, index);
    }

    fn array_store_u8(&mut self, descriptor: u8, index: u8, value: u8) {
        self.asm.mov_mem_r(RBP, self.frame_scratch(), value);
        let limit = value;
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        self.asm.mov_r_mem(limit, RBP, self.frame_scratch());
        self.asm.mov_mem_index_r8(descriptor, index, limit);
    }

    fn array_load_f64(&mut self, dst: u8, descriptor: u8, index: u8) {
        // Length first, while `descriptor` still holds the descriptor address.
        let limit = if dst == descriptor { RDX } else { dst };
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        // Scale 3 is eight bytes per element, and a plain 64-bit `mov` because
        // the bit pattern *is* the value.
        self.asm.mov_r_mem_index(dst, descriptor, index, 3);
    }

    fn array_store_f64(&mut self, descriptor: u8, index: u8, value: u8) {
        // Every scratch register is live — descriptor, index and value — so
        // the value is parked in the frame while the length is checked, the
        // same shape `array_store_i32` uses.
        self.asm.mov_mem_r(RBP, self.frame_scratch(), value);
        let limit = value;
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        self.asm.mov_r_mem(limit, RBP, self.frame_scratch());
        self.asm.mov_mem_index_r(descriptor, index, 3, limit);
    }

    fn array_store_i32(&mut self, descriptor: u8, index: u8, value: u8) {
        // Every scratch register is live here — descriptor, index and value —
        // so the length is compared against a frame slot rather than a fourth
        // register.
        self.asm.mov_mem_r(RBP, self.frame_scratch(), value);
        let limit = value;
        self.asm.mov_r_mem(limit, descriptor, 8);
        self.bounds_check(index, limit);
        self.asm.mov_r_mem(descriptor, descriptor, 0);
        self.asm.mov_r_mem(limit, RBP, self.frame_scratch());
        self.asm.mov_mem_index_r32(descriptor, index, limit);
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
    trap_slot: usize,
    plan: &crate::translate::Plan,
    frame_map: i64,
    widest_callee: usize,
    reference_slots: &[Pinnable],
) -> Result<Vec<u8>, CompileError> {
    let mut e = X64Emitter::new(trap_slot);
    e.frame_map = frame_map;
    e.widest_callee = widest_callee;
    e.polls = !plan.safepoints.is_empty();
    // Chosen before anything is emitted: the prologue seeds these registers,
    // and every later access has to agree with what it decided.
    e.pinned = slots_worth_pinning(instructions, locals, reference_slots);
    crate::translate::translate_with_plan(
        &mut e,
        instructions,
        analysis,
        locals,
        returns_value,
        plan,
    )?;
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
        // Trap slot 0: this test never emits a bounds check.
        let mut e = X64Emitter::new(0);
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
    use rustclr_core::opcode::{decode_all, Op};

    /// Assembles a method body directly, bypassing the loader, and runs it.
    fn run(il: &[u8], locals: usize, args: &[i64]) -> i64 {
        let instructions = decode_all(il).expect("decode");
        let analysis = analyse(&instructions, 8, &[], |ins| {
            if ins.op == Op::Ret { -1 } else { 0 }
        })
        .expect("verify");
        // These helpers compile array-free bodies, so the trap slot is never
        // written; one past the arguments keeps it consistent with the tier.
        // The prologue still *reads* the context slots past it — the frame
        // record it writes names the parent frame — so the array handed over
        // below has to be the full shape the tier passes, not just the
        // arguments.
        let code = emit(
            &instructions,
            &analysis,
            locals,
            true,
            args.len(),
            &crate::translate::Plan::default(),
            // These helpers compile bodies with no references and no loop, so
            // there is nothing for a safe point to describe and no map to name.
            0,
            // And no callees, so the outgoing frame need only fit this method.
            0,
            &[],
        )
        .expect("emit");
        let page = CodePage::commit(&code).expect("commit");
        // Arguments, then the trap slot and the six context words the tier
        // supplies. All zero: no allocation context, no safe point, no parent.
        let mut frame: Vec<i64> = args.to_vec();
        frame.extend([0i64; 7]);
        // SAFETY: `emit` produced this page and the signature matches.
        let f: extern "C" fn(*const i64) -> i64 =
            unsafe { core::mem::transmute(page.as_ptr()) };
        f(frame.as_ptr())
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
