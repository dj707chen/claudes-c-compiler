//! Function call helpers for AArch64 (AAPCS64).
//!
//! Handles argument register/stack emission, parameter storage from incoming
//! registers, variadic register saves, and F128 argument pre-conversion.

use crate::ir::ir::{IrConst, IrFunction, Operand, Value};
use crate::common::types::IrType;
use crate::backend::state::StackSlot;
use crate::backend::call_abi::CallArgClass;
use crate::backend::call_emit::ParamClass;
use super::codegen::{ArmCodegen, callee_saved_name, ARM_ARG_REGS, ARM_TMP_REGS};
use crate::backend::generation::find_param_alloca;

impl ArmCodegen {
    /// Load an operand into the given destination register, accounting for SP adjustment.
    /// When `needs_adjusted_load` is true, values must be loaded from adjusted stack offsets
    /// or callee-saved registers (since SP has been modified for stack args).
    pub(super) fn emit_load_arg_to_reg(&mut self, arg: &Operand, dest: &str, slot_adjust: i64, extra_sp_adj: i64, needs_adjusted_load: bool) {
        if needs_adjusted_load || extra_sp_adj > 0 {
            match arg {
                Operand::Value(v) => {
                    if let Some(&reg) = self.reg_assignments.get(&v.0) {
                        self.state.emit_fmt(format_args!("    mov {}, {}", dest, callee_saved_name(reg)));
                    } else if let Some(slot) = self.state.get_slot(v.0) {
                        let adjusted = slot.0 + slot_adjust + extra_sp_adj;
                        if self.state.is_alloca(v.0) {
                            self.emit_add_sp_offset(dest, adjusted);
                        } else {
                            self.emit_load_from_sp(dest, adjusted, "ldr");
                        }
                    } else {
                        self.state.emit_fmt(format_args!("    mov {}, #0", dest));
                    }
                }
                Operand::Const(_) => {
                    self.operand_to_x0(arg);
                    if dest != "x0" {
                        self.state.emit_fmt(format_args!("    mov {}, x0", dest));
                    }
                }
            }
        } else {
            // For Value operands, load directly into dest to avoid clobbering x0.
            // The operand_to_x0 path unconditionally uses x0 as scratch, which
            // can destroy previously-loaded argument registers (e.g., when struct
            // arguments are reordered in a call like check(y, x)).
            match arg {
                Operand::Value(v) => {
                    if let Some(&reg) = self.reg_assignments.get(&v.0) {
                        self.state.emit_fmt(format_args!("    mov {}, {}", dest, callee_saved_name(reg)));
                    } else if let Some(slot) = self.state.get_slot(v.0) {
                        if self.state.is_alloca(v.0) {
                            self.emit_add_sp_offset(dest, slot.0);
                        } else {
                            self.emit_load_from_sp(dest, slot.0, "ldr");
                        }
                    } else {
                        self.state.emit_fmt(format_args!("    mov {}, #0", dest));
                    }
                }
                Operand::Const(_) => {
                    self.operand_to_x0(arg);
                    if dest != "x0" {
                        self.state.emit_fmt(format_args!("    mov {}, x0", dest));
                    }
                }
            }
        }
    }

    /// Phase 2a: Load GP integer register args into temp registers (x9-x16).
    pub(super) fn emit_call_gp_to_temps(&mut self, args: &[Operand], arg_classes: &[CallArgClass],
                              slot_adjust: i64, needs_adjusted_load: bool) {
        let mut gp_tmp_idx = 0usize;
        for (i, arg) in args.iter().enumerate() {
            if !matches!(arg_classes[i], CallArgClass::IntReg { .. }) { continue; }
            if gp_tmp_idx >= 8 { break; }
            self.emit_load_arg_to_reg(arg, "x0", slot_adjust, 0, needs_adjusted_load);
            self.state.emit_fmt(format_args!("    mov {}, x0", ARM_TMP_REGS[gp_tmp_idx]));
            gp_tmp_idx += 1;
        }
    }

    /// Phase 2b: Load FP register args, handling F128 via temp stack + __extenddftf2.
    pub(super) fn emit_call_fp_reg_args(&mut self, args: &[Operand], arg_classes: &[CallArgClass],
                              arg_types: &[IrType], slot_adjust: i64, needs_adjusted_load: bool) {
        let fp_reg_assignments: Vec<(usize, usize)> = args.iter().enumerate()
            .filter(|(i, _)| matches!(arg_classes[*i], CallArgClass::FloatReg { .. } | CallArgClass::F128Reg { .. }))
            .map(|(i, _)| {
                let reg_idx = match arg_classes[i] {
                    CallArgClass::FloatReg { reg_idx } | CallArgClass::F128Reg { reg_idx } => reg_idx,
                    _ => 0,
                };
                (i, reg_idx)
            })
            .collect();

        let f128_var_count: usize = fp_reg_assignments.iter()
            .filter(|&&(arg_i, _)| matches!(arg_classes[arg_i], CallArgClass::F128Reg { .. }) && matches!(&args[arg_i], Operand::Value(_)))
            .count();
        let f128_temp_space_aligned = (f128_var_count * 16 + 15) & !15;
        if f128_temp_space_aligned > 0 {
            self.emit_sub_sp(f128_temp_space_aligned as i64);
        }

        let extra_sp_adj = f128_temp_space_aligned as i64;
        let f128_temp_slots = self.emit_call_f128_var_args(
            args, arg_classes, &fp_reg_assignments, slot_adjust, extra_sp_adj, needs_adjusted_load,
        );

        self.emit_call_f128_const_args(args, arg_classes, &fp_reg_assignments);

        for &(reg_i, temp_off) in &f128_temp_slots {
            self.state.emit_fmt(format_args!("    ldr q{}, [sp, #{}]", reg_i, temp_off));
        }
        if f128_temp_space_aligned > 0 {
            self.emit_add_sp(f128_temp_space_aligned as i64);
        }

        for &(arg_i, reg_i) in &fp_reg_assignments {
            if matches!(arg_classes[arg_i], CallArgClass::F128Reg { .. }) { continue; }
            let arg_ty = if arg_i < arg_types.len() { Some(arg_types[arg_i]) } else { None };
            self.emit_load_arg_to_reg(&args[arg_i], "x0", slot_adjust, 0, needs_adjusted_load);
            if arg_ty == Some(IrType::F32) {
                self.state.emit_fmt(format_args!("    fmov s{}, w0", reg_i));
            } else {
                self.state.emit_fmt(format_args!("    fmov d{}, x0", reg_i));
            }
        }
    }

    /// Convert F128 variable args to full-precision f128, saving to temp stack.
    pub(super) fn emit_call_f128_var_args(&mut self, args: &[Operand], arg_classes: &[CallArgClass],
                                fp_reg_assignments: &[(usize, usize)],
                                slot_adjust: i64, extra_sp_adj: i64,
                                needs_adjusted_load: bool) -> Vec<(usize, usize)> {
        let mut f128_temp_idx = 0usize;
        let mut f128_temp_slots: Vec<(usize, usize)> = Vec::new();
        for &(arg_i, reg_i) in fp_reg_assignments {
            if !matches!(arg_classes[arg_i], CallArgClass::F128Reg { .. }) { continue; }
            if let Operand::Value(v) = &args[arg_i] {
                let temp_off = f128_temp_idx * 16;
                let loaded_full = self.try_load_f128_full_precision(v.0, slot_adjust + extra_sp_adj, temp_off);

                if !loaded_full {
                    self.emit_load_arg_to_reg(&args[arg_i], "x0", slot_adjust, extra_sp_adj,
                        needs_adjusted_load || extra_sp_adj > 0);
                    self.state.emit("    fmov d0, x0");
                    self.state.emit("    stp x9, x10, [sp, #-16]!");
                    self.state.emit("    bl __extenddftf2");
                    self.state.emit("    ldp x9, x10, [sp], #16");
                    self.state.emit_fmt(format_args!("    str q0, [sp, #{}]", temp_off));
                }

                f128_temp_slots.push((reg_i, temp_off));
                f128_temp_idx += 1;
            }
        }
        f128_temp_slots
    }

    /// Try to load a full-precision f128 value via f128 tracking. Returns true if successful.
    pub(super) fn try_load_f128_full_precision(&mut self, value_id: u32, adjusted_slot_base: i64, temp_off: usize) -> bool {
        if let Some((src_id, offset, is_indirect)) = self.state.get_f128_source(value_id) {
            if !is_indirect {
                if let Some(src_slot) = self.state.get_slot(src_id) {
                    let adj = src_slot.0 + offset + adjusted_slot_base;
                    self.emit_load_from_sp("q0", adj, "ldr");
                    self.state.emit_fmt(format_args!("    str q0, [sp, #{}]", temp_off));
                    return true;
                }
            } else if let Some(src_slot) = self.state.get_slot(src_id) {
                let adj = src_slot.0 + adjusted_slot_base;
                self.emit_load_from_sp("x17", adj, "ldr");
                if offset != 0 {
                    if offset > 0 && offset <= 4095 {
                        self.state.emit_fmt(format_args!("    add x17, x17, #{}", offset));
                    } else {
                        self.load_large_imm("x16", offset);
                        self.state.emit("    add x17, x17, x16");
                    }
                }
                self.state.emit("    ldr q0, [x17]");
                self.state.emit_fmt(format_args!("    str q0, [sp, #{}]", temp_off));
                return true;
            }
        }
        false
    }

    /// Load F128 constants directly into target Q registers using full f128 bytes.
    pub(super) fn emit_call_f128_const_args(&mut self, args: &[Operand], arg_classes: &[CallArgClass],
                                  fp_reg_assignments: &[(usize, usize)]) {
        for &(arg_i, reg_i) in fp_reg_assignments {
            if !matches!(arg_classes[arg_i], CallArgClass::F128Reg { .. }) { continue; }
            if let Operand::Const(c) = &args[arg_i] {
                let bytes = match c {
                    IrConst::LongDouble(_, f128_bytes) => *f128_bytes,
                    _ => {
                        let f64_val = c.to_f64().unwrap_or(0.0);
                        crate::ir::ir::f64_to_f128_bytes(f64_val)
                    }
                };
                let lo = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                let hi = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                self.emit_load_imm64("x0", lo as i64);
                self.emit_load_imm64("x1", hi as i64);
                self.state.emit("    stp x0, x1, [sp, #-16]!");
                self.state.emit_fmt(format_args!("    ldr q{}, [sp]", reg_i));
                self.state.emit("    add sp, sp, #16");
            }
        }
    }

    /// Phase 3: Move GP int args from temp regs to actual arg registers.
    pub(super) fn emit_call_move_temps_to_arg_regs(&mut self, args: &[Operand], arg_classes: &[CallArgClass]) {
        let mut int_reg_idx = 0usize;
        let mut gp_tmp_idx = 0usize;
        for (i, _) in args.iter().enumerate() {
            match arg_classes[i] {
                CallArgClass::I128RegPair { .. } => {
                    if !int_reg_idx.is_multiple_of(2) { int_reg_idx += 1; }
                    int_reg_idx += 2;
                }
                CallArgClass::StructByValReg { size, .. } => {
                    int_reg_idx += if size <= 8 { 1 } else { 2 };
                }
                CallArgClass::IntReg { .. } => {
                    if gp_tmp_idx < 8 && int_reg_idx < 8 {
                        self.state.emit_fmt(format_args!("    mov {}, {}", ARM_ARG_REGS[int_reg_idx], ARM_TMP_REGS[gp_tmp_idx]));
                        int_reg_idx += 1;
                    }
                    gp_tmp_idx += 1;
                }
                _ => {}
            }
        }
    }

    /// Phase 3b: Load i128 register pair args into paired arg registers.
    pub(super) fn emit_call_i128_reg_args(&mut self, args: &[Operand], arg_classes: &[CallArgClass],
                                slot_adjust: i64, needs_adjusted_load: bool) {
        for (i, arg) in args.iter().enumerate() {
            if let CallArgClass::I128RegPair { base_reg_idx } = arg_classes[i] {
                match arg {
                    Operand::Value(v) => {
                        if let Some(slot) = self.state.get_slot(v.0) {
                            let adj = if needs_adjusted_load { slot.0 + slot_adjust } else { slot.0 };
                            if self.state.is_alloca(v.0) {
                                if needs_adjusted_load {
                                    self.emit_load_from_sp(ARM_ARG_REGS[base_reg_idx], adj, "ldr");
                                } else {
                                    self.emit_add_sp_offset(ARM_ARG_REGS[base_reg_idx], adj);
                                }
                                self.state.emit_fmt(format_args!("    mov {}, #0", ARM_ARG_REGS[base_reg_idx + 1]));
                            } else {
                                self.emit_load_from_sp(ARM_ARG_REGS[base_reg_idx], adj, "ldr");
                                self.emit_load_from_sp(ARM_ARG_REGS[base_reg_idx + 1], adj + 8, "ldr");
                            }
                        }
                    }
                    Operand::Const(c) => {
                        if let IrConst::I128(v) = c {
                            self.emit_load_imm64(ARM_ARG_REGS[base_reg_idx], *v as u64 as i64);
                            self.emit_load_imm64(ARM_ARG_REGS[base_reg_idx + 1], (*v >> 64) as u64 as i64);
                        } else {
                            self.operand_to_x0(arg);
                            if base_reg_idx != 0 {
                                self.state.emit_fmt(format_args!("    mov {}, x0", ARM_ARG_REGS[base_reg_idx]));
                            }
                            self.state.emit_fmt(format_args!("    mov {}, #0", ARM_ARG_REGS[base_reg_idx + 1]));
                        }
                    }
                }
            }
        }
    }

    /// Phase 3c: Load struct-by-value register args. Loads pointer into x17,
    /// then reads struct data from [x17] into arg regs.
    pub(super) fn emit_call_struct_byval_reg_args(&mut self, args: &[Operand], arg_classes: &[CallArgClass],
                                        slot_adjust: i64, needs_adjusted_load: bool) {
        for (i, arg) in args.iter().enumerate() {
            if let CallArgClass::StructByValReg { base_reg_idx, size } = arg_classes[i] {
                let regs_needed = if size <= 8 { 1 } else { 2 };
                self.emit_load_arg_to_reg(arg, "x17", slot_adjust, 0, needs_adjusted_load);
                self.state.emit_fmt(format_args!("    ldr {}, [x17]", ARM_ARG_REGS[base_reg_idx]));
                if regs_needed > 1 {
                    self.state.emit_fmt(format_args!("    ldr {}, [x17, #8]", ARM_ARG_REGS[base_reg_idx + 1]));
                }
            }
        }
    }

    /// Resolve param alloca to (slot, type) for parameter `i`.
    pub(super) fn resolve_param_slot(&self, func: &IrFunction, i: usize) -> Option<(StackSlot, IrType, Value)> {
        let (dest, ty) = find_param_alloca(func, i)?;
        let slot = self.state.get_slot(dest.0)?;
        Some((slot, ty, dest))
    }

    /// Save variadic function registers to save areas.
    pub(super) fn emit_save_variadic_regs(&mut self) {
        let gp_base = self.va_gp_save_offset;
        for i in (0..8).step_by(2) {
            let offset = gp_base + (i as i64) * 8;
            self.emit_stp_to_sp(&format!("x{}", i), &format!("x{}", i + 1), offset);
        }
        if !self.general_regs_only {
            let fp_base = self.va_fp_save_offset;
            for i in (0..8).step_by(2) {
                let offset = fp_base + (i as i64) * 16;
                self.emit_stp_to_sp(&format!("q{}", i), &format!("q{}", i + 1), offset);
            }
        }
    }

    /// Phase 1: Store GP register params to alloca slots.
    pub(super) fn emit_store_gp_params(&mut self, func: &IrFunction, param_classes: &[ParamClass]) {
        for (i, _) in func.params.iter().enumerate() {
            let class = param_classes[i];
            if !class.uses_gp_reg() { continue; }

            let (slot, ty, _) = match self.resolve_param_slot(func, i) {
                Some(v) => v,
                None => continue,
            };

            match class {
                ParamClass::IntReg { reg_idx } => {
                    let store_instr = Self::str_for_type(ty);
                    let reg = Self::reg_for_type(ARM_ARG_REGS[reg_idx], ty);
                    self.emit_store_to_sp(reg, slot.0, store_instr);
                }
                ParamClass::I128RegPair { base_reg_idx } => {
                    self.emit_store_to_sp(ARM_ARG_REGS[base_reg_idx], slot.0, "str");
                    self.emit_store_to_sp(ARM_ARG_REGS[base_reg_idx + 1], slot.0 + 8, "str");
                }
                ParamClass::StructByValReg { base_reg_idx, size } => {
                    self.emit_store_to_sp(ARM_ARG_REGS[base_reg_idx], slot.0, "str");
                    if size > 8 {
                        self.emit_store_to_sp(ARM_ARG_REGS[base_reg_idx + 1], slot.0 + 8, "str");
                    }
                }
                ParamClass::LargeStructByRefReg { reg_idx, size } => {
                    let src_reg = ARM_ARG_REGS[reg_idx];
                    let n_dwords = size.div_ceil(8);
                    for qi in 0..n_dwords {
                        let src_off = (qi * 8) as i64;
                        self.emit_load_from_reg("x9", src_reg, src_off, "ldr");
                        self.emit_store_to_sp("x9", slot.0 + src_off, "str");
                    }
                }
                _ => {}
            }
        }
    }

    /// Phase 2: Store FP register params to alloca slots.
    pub(super) fn emit_store_fp_params(&mut self, func: &IrFunction, param_classes: &[ParamClass]) {
        let has_f128_fp_params = param_classes.iter().enumerate().any(|(i, c)| {
            matches!(c, ParamClass::F128FpReg { .. }) &&
            find_param_alloca(func, i).is_some()
        });

        if has_f128_fp_params {
            self.emit_store_fp_params_with_f128(func, param_classes);
        } else {
            self.emit_store_fp_params_simple(func, param_classes);
        }
    }

    /// Store FP params when F128 params are present (save/restore q0-q7).
    pub(super) fn emit_store_fp_params_with_f128(&mut self, func: &IrFunction, param_classes: &[ParamClass]) {
        self.emit_sub_sp(128);
        for i in 0..8usize {
            self.state.emit_fmt(format_args!("    str q{}, [sp, #{}]", i, i * 16));
        }

        // Process non-F128 float params first (from saved Q area).
        for (i, _) in func.params.iter().enumerate() {
            let reg_idx = match param_classes[i] {
                ParamClass::FloatReg { reg_idx } => reg_idx,
                _ => continue,
            };
            let (slot, ty, _) = match self.resolve_param_slot(func, i) {
                Some(v) => v,
                None => continue,
            };
            let fp_reg_off = (reg_idx * 16) as i64;
            if ty == IrType::F32 {
                self.state.emit_fmt(format_args!("    ldr s0, [sp, #{}]", fp_reg_off));
                self.state.emit("    fmov w0, s0");
            } else {
                self.state.emit_fmt(format_args!("    ldr d0, [sp, #{}]", fp_reg_off));
                self.state.emit("    fmov x0, d0");
            }
            self.emit_store_to_sp("x0", slot.0 + 128, "str");
        }

        // Process F128 FP reg params: store full 16-byte f128, then f64 approx.
        for (i, _) in func.params.iter().enumerate() {
            let reg_idx = match param_classes[i] {
                ParamClass::F128FpReg { reg_idx } => reg_idx,
                _ => continue,
            };
            let (slot, _, dest_val) = match self.resolve_param_slot(func, i) {
                Some(v) => v,
                None => continue,
            };
            let fp_reg_off = (reg_idx * 16) as i64;
            self.state.emit_fmt(format_args!("    ldr q0, [sp, #{}]", fp_reg_off));
            self.emit_store_to_sp("q0", slot.0 + 128, "str");
            self.state.track_f128_self(dest_val.0);
            self.state.emit("    bl __trunctfdf2");
            self.state.emit("    fmov x0, d0");
        }

        self.emit_add_sp(128);
    }

    /// Store FP params when no F128 params are present (simple path).
    pub(super) fn emit_store_fp_params_simple(&mut self, func: &IrFunction, param_classes: &[ParamClass]) {
        for (i, _) in func.params.iter().enumerate() {
            let reg_idx = match param_classes[i] {
                ParamClass::FloatReg { reg_idx } => reg_idx,
                _ => continue,
            };
            let (slot, ty, _) = match self.resolve_param_slot(func, i) {
                Some(v) => v,
                None => continue,
            };
            if ty == IrType::F32 {
                self.state.emit_fmt(format_args!("    fmov w0, s{}", reg_idx));
            } else {
                self.state.emit_fmt(format_args!("    fmov x0, d{}", reg_idx));
            }
            self.emit_store_to_sp("x0", slot.0, "str");
        }
    }

    /// Phase 3: Store stack-passed params to alloca slots.
    pub(super) fn emit_store_stack_params(&mut self, func: &IrFunction, param_classes: &[ParamClass]) {
        let frame_size = self.current_frame_size;
        for (i, _) in func.params.iter().enumerate() {
            let class = param_classes[i];
            if !class.is_stack() { continue; }

            let (slot, ty, dest_val) = match self.resolve_param_slot(func, i) {
                Some(v) => v,
                None => continue,
            };

            match class {
                ParamClass::StructStack { offset, size } | ParamClass::LargeStructStack { offset, size } => {
                    let caller_offset = frame_size + offset;
                    for qi in 0..size.div_ceil(8) {
                        let off = qi as i64 * 8;
                        self.emit_load_from_sp("x0", caller_offset + off, "ldr");
                        self.emit_store_to_sp("x0", slot.0 + off, "str");
                    }
                }
                ParamClass::F128Stack { offset } => {
                    let caller_offset = frame_size + offset;
                    self.emit_load_from_sp("x0", caller_offset, "ldr");
                    self.emit_store_to_sp("x0", slot.0, "str");
                    self.emit_load_from_sp("x0", caller_offset + 8, "ldr");
                    self.emit_store_to_sp("x0", slot.0 + 8, "str");
                    self.state.track_f128_self(dest_val.0);
                }
                ParamClass::I128Stack { offset } => {
                    let caller_offset = frame_size + offset;
                    self.emit_load_from_sp("x0", caller_offset, "ldr");
                    self.emit_store_to_sp("x0", slot.0, "str");
                    self.emit_load_from_sp("x0", caller_offset + 8, "ldr");
                    self.emit_store_to_sp("x0", slot.0 + 8, "str");
                }
                ParamClass::StackScalar { offset } => {
                    let caller_offset = frame_size + offset;
                    self.emit_load_from_sp("x0", caller_offset, "ldr");
                    let store_instr = Self::str_for_type(ty);
                    let reg = Self::reg_for_type("x0", ty);
                    self.emit_store_to_sp(reg, slot.0, store_instr);
                }
                ParamClass::LargeStructByRefStack { offset, size } => {
                    let caller_offset = frame_size + offset;
                    self.emit_load_from_sp("x0", caller_offset, "ldr");
                    for qi in 0..size.div_ceil(8) {
                        let off = (qi * 8) as i64;
                        self.emit_load_from_reg("x1", "x0", off, "ldr");
                        self.emit_store_to_sp("x1", slot.0 + off, "str");
                    }
                }
                _ => {}
            }
        }
    }

}
