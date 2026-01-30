//! Core operand loading and register manipulation helpers for x86-64.
//!
//! These methods handle loading IR operands into physical registers (rax, rcx),
//! storing results back to stack slots or register assignments, and mapping
//! IR types to instruction mnemonics and register sub-names.

use crate::ir::ir::{IrConst, Operand, Value};
use crate::common::types::IrType;
use crate::backend::regalloc::PhysReg;
use super::codegen::{X86Codegen, phys_reg_name, phys_reg_name_32, reg_name_to_32};

impl X86Codegen {
    /// Get the callee-saved register assigned to an operand, if any.
    pub(super) fn operand_reg(&self, op: &Operand) -> Option<PhysReg> {
        match op {
            Operand::Value(v) => self.reg_assignments.get(&v.0).copied(),
            _ => None,
        }
    }

    /// Get the callee-saved register assigned to a destination value, if any.
    pub(super) fn dest_reg(&self, dest: &Value) -> Option<PhysReg> {
        self.reg_assignments.get(&dest.0).copied()
    }

    /// Emit sign-extension from 32-bit to 64-bit register if the type is signed.
    /// Used after 32-bit ALU operations on callee-saved registers.
    pub(super) fn emit_sext32_if_needed(&mut self, name_32: &str, name_64: &str, is_unsigned: bool) {
        if !is_unsigned {
            self.state.out.emit_instr_reg_reg("    movslq", name_32, name_64);
        }
    }

    /// Load an operand into a specific callee-saved register.
    /// Handles constants, register-allocated values, and stack values.
    pub(super) fn operand_to_callee_reg(&mut self, op: &Operand, target: PhysReg) {
        let target_name = phys_reg_name(target);
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) if *v == 0 => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", phys_reg_name_32(target))),
                    IrConst::I16(v) if *v == 0 => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", phys_reg_name_32(target))),
                    IrConst::I32(v) if *v == 0 => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", phys_reg_name_32(target))),
                    IrConst::I64(0) => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", phys_reg_name_32(target))),
                    IrConst::I8(v) => self.state.emit_fmt(format_args!("    movq ${}, %{}", *v as i64, target_name)),
                    IrConst::I16(v) => self.state.emit_fmt(format_args!("    movq ${}, %{}", *v as i64, target_name)),
                    IrConst::I32(v) => self.state.emit_fmt(format_args!("    movq ${}, %{}", *v as i64, target_name)),
                    IrConst::I64(v) => {
                        if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", *v, target_name);
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", *v, target_name);
                        }
                    }
                    IrConst::Zero => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", phys_reg_name_32(target))),
                    _ => {
                        // For float/i128 constants, fall back to loading to rax and moving
                        self.operand_to_rax(op);
                        self.state.out.emit_instr_reg_reg("    movq", "rax", target_name);
                    }
                }
            }
            Operand::Value(v) => {
                if let Some(&reg) = self.reg_assignments.get(&v.0) {
                    if reg.0 != target.0 {
                        let src_name = phys_reg_name(reg);
                        self.state.out.emit_instr_reg_reg("    movq", src_name, target_name);
                    }
                    // If same register, nothing to do
                } else if let Some(slot) = self.state.get_slot(v.0) {
                    if self.state.is_alloca(v.0) {
                        self.state.out.emit_instr_rbp_reg("    leaq", slot.0, target_name);
                    } else {
                        self.state.out.emit_instr_rbp_reg("    movq", slot.0, target_name);
                    }
                } else if self.state.reg_cache.acc_has(v.0, false) || self.state.reg_cache.acc_has(v.0, true) {
                    self.state.out.emit_instr_reg_reg("    movq", "rax", target_name);
                } else {
                    let target_32 = phys_reg_name_32(target);
                    self.state.out.emit_instr_reg_reg("    xorl", target_32, target_32);
                }
            }
        }
    }

    /// Load an operand into %rax. Uses the register cache to skip the load
    /// if the value is already in %rax.
    pub(super) fn operand_to_rax(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                self.state.reg_cache.invalidate_acc();
                match c {
                    IrConst::I8(v) if *v == 0 => self.state.emit("    xorl %eax, %eax"),
                    IrConst::I16(v) if *v == 0 => self.state.emit("    xorl %eax, %eax"),
                    IrConst::I32(v) if *v == 0 => self.state.emit("    xorl %eax, %eax"),
                    IrConst::I64(0) => self.state.emit("    xorl %eax, %eax"),
                    IrConst::I8(v) => self.state.out.emit_instr_imm_reg("    movq", *v as i64, "rax"),
                    IrConst::I16(v) => self.state.out.emit_instr_imm_reg("    movq", *v as i64, "rax"),
                    IrConst::I32(v) => self.state.out.emit_instr_imm_reg("    movq", *v as i64, "rax"),
                    IrConst::I64(v) => {
                        if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", *v, "rax");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", *v, "rax");
                        }
                    }
                    IrConst::F32(v) => {
                        let bits = v.to_bits() as u64;
                        if bits == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movq", bits as i64, "rax");
                        }
                    }
                    IrConst::F64(v) => {
                        let bits = v.to_bits();
                        if bits == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", bits as i64, "rax");
                        }
                    }
                    // LongDouble at computation level is treated as F64
                    IrConst::LongDouble(v, _) => {
                        let bits = v.to_bits();
                        if bits == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", bits as i64, "rax");
                        }
                    }
                    IrConst::I128(v) => {
                        // Truncate to low 64 bits for rax-only path
                        let low = *v as i64;
                        if low == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else if low >= i32::MIN as i64 && low <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", low, "rax");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", low, "rax");
                        }
                    }
                    IrConst::Zero => self.state.emit("    xorl %eax, %eax"),
                }
            }
            Operand::Value(v) => {
                let is_alloca = self.state.is_alloca(v.0);
                // Check cache: skip load if value is already in %rax
                if self.state.reg_cache.acc_has(v.0, is_alloca) {
                    return;
                }
                // Check register allocation: load from callee-saved register
                if let Some(&reg) = self.reg_assignments.get(&v.0) {
                    let reg_name = phys_reg_name(reg);
                    self.state.out.emit_instr_reg_reg("    movq", reg_name, "rax");
                    self.state.reg_cache.set_acc(v.0, false);
                } else if self.state.get_slot(v.0).is_some() {
                    self.value_to_reg(v, "rax");
                    self.state.reg_cache.set_acc(v.0, is_alloca);
                } else {
                    self.state.emit("    xorl %eax, %eax");
                    self.state.reg_cache.invalidate_acc();
                }
            }
        }
    }

    /// Store %rax to a value's location (register or stack slot).
    /// Register-only strategy: if the value has a register assignment (callee-saved or caller-saved),
    /// store ONLY to the register (skip the stack write). This eliminates redundant
    /// memory stores for register-allocated values. Values without a register
    /// assignment are stored to their stack slot as before.
    pub(super) fn store_rax_to(&mut self, dest: &Value) {
        if let Some(&reg) = self.reg_assignments.get(&dest.0) {
            // Value has a callee-saved register: store only to register, skip stack.
            let reg_name = phys_reg_name(reg);
            self.state.out.emit_instr_reg_reg("    movq", "rax", reg_name);
        } else if let Some(slot) = self.state.get_slot(dest.0) {
            // No register: store to stack slot.
            self.state.out.emit_instr_reg_rbp("    movq", "rax", slot.0);
        }
        // After storing to dest, %rax still holds dest's value
        self.state.reg_cache.set_acc(dest.0, false);
    }

    /// Load an operand directly into %rcx, avoiding the push/pop pattern.
    /// This is the key optimization: instead of loading to rax, pushing, loading
    /// the other operand to rax, moving rax->rcx, then popping rax, we load
    /// directly to rcx with a single instruction.
    pub(super) fn operand_to_rcx(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) if *v == 0 => self.state.emit("    xorl %ecx, %ecx"),
                    IrConst::I16(v) if *v == 0 => self.state.emit("    xorl %ecx, %ecx"),
                    IrConst::I32(v) if *v == 0 => self.state.emit("    xorl %ecx, %ecx"),
                    IrConst::I64(0) => self.state.emit("    xorl %ecx, %ecx"),
                    IrConst::I8(v) => self.state.out.emit_instr_imm_reg("    movq", *v as i64, "rcx"),
                    IrConst::I16(v) => self.state.out.emit_instr_imm_reg("    movq", *v as i64, "rcx"),
                    IrConst::I32(v) => self.state.out.emit_instr_imm_reg("    movq", *v as i64, "rcx"),
                    IrConst::I64(v) => {
                        if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", *v, "rcx");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", *v, "rcx");
                        }
                    }
                    IrConst::F32(v) => {
                        let bits = v.to_bits() as u64;
                        if bits == 0 {
                            self.state.emit("    xorl %ecx, %ecx");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movq", bits as i64, "rcx");
                        }
                    }
                    IrConst::F64(v) => {
                        let bits = v.to_bits();
                        if bits == 0 {
                            self.state.emit("    xorl %ecx, %ecx");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", bits as i64, "rcx");
                        }
                    }
                    IrConst::LongDouble(v, _) => {
                        let bits = v.to_bits();
                        if bits == 0 {
                            self.state.emit("    xorl %ecx, %ecx");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", bits as i64, "rcx");
                        }
                    }
                    IrConst::I128(v) => {
                        let low = *v as i64;
                        if low == 0 {
                            self.state.emit("    xorl %ecx, %ecx");
                        } else if low >= i32::MIN as i64 && low <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", low, "rcx");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", low, "rcx");
                        }
                    }
                    IrConst::Zero => self.state.emit("    xorl %ecx, %ecx"),
                }
            }
            Operand::Value(v) => {
                // Check register allocation: load from callee-saved register
                if let Some(&reg) = self.reg_assignments.get(&v.0) {
                    let reg_name = phys_reg_name(reg);
                    self.state.out.emit_instr_reg_reg("    movq", reg_name, "rcx");
                } else if self.state.get_slot(v.0).is_some() {
                    self.value_to_reg(v, "rcx");
                } else if self.state.reg_cache.acc_has(v.0, false) || self.state.reg_cache.acc_has(v.0, true) {
                    self.state.out.emit_instr_reg_reg("    movq", "rax", "rcx");
                } else {
                    self.state.emit("    xorl %ecx, %ecx");
                }
            }
        }
    }

    /// Check if an operand is a small constant that fits in a 32-bit immediate.
    /// Returns the immediate value if it fits, None otherwise.
    pub(super) fn const_as_imm32(op: &Operand) -> Option<i64> {
        match op {
            Operand::Const(c) => {
                let val = match c {
                    IrConst::I8(v) => *v as i64,
                    IrConst::I16(v) => *v as i64,
                    IrConst::I32(v) => *v as i64,
                    IrConst::I64(v) => *v,
                    IrConst::Zero => 0,
                    _ => return None,
                };
                if val >= i32::MIN as i64 && val <= i32::MAX as i64 {
                    Some(val)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Load a Value into a named register. For allocas, loads the address (leaq);
    /// for register-allocated values, copies from the callee-saved register;
    /// for regular values, loads the data (movq) from the stack slot.
    pub(super) fn value_to_reg(&mut self, val: &Value, reg: &str) {
        // Check register allocation first (allocas are never register-allocated)
        if let Some(&phys_reg) = self.reg_assignments.get(&val.0) {
            let reg_name = phys_reg_name(phys_reg);
            if reg_name != reg {
                self.state.out.emit_instr_reg_reg("    movq", reg_name, reg);
            }
            return;
        }
        if let Some(slot) = self.state.get_slot(val.0) {
            if self.state.is_alloca(val.0) {
                if let Some(align) = self.state.alloca_over_align(val.0) {
                    // Over-aligned alloca: compute aligned address within the
                    // oversized stack slot. The slot has (align - 1) extra bytes
                    // to guarantee we can find an aligned address within it.
                    self.state.out.emit_instr_rbp_reg("    leaq", slot.0, reg);
                    self.state.out.emit_instr_imm_reg("    addq", (align - 1) as i64, reg);
                    self.state.out.emit_instr_imm_reg("    andq", -(align as i64), reg);
                } else {
                    self.state.out.emit_instr_rbp_reg("    leaq", slot.0, reg);
                }
            } else {
                self.state.out.emit_instr_rbp_reg("    movq", slot.0, reg);
            }
        }
    }

    /// Get the store instruction mnemonic for a given type.
    pub(super) fn mov_store_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "movb",
            IrType::I16 | IrType::U16 => "movw",
            IrType::I32 | IrType::U32 | IrType::F32 => "movl",
            _ => "movq",
        }
    }

    /// Get the load instruction mnemonic for a given type.
    pub(super) fn mov_load_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 => "movsbq",
            IrType::U8 => "movzbq",
            IrType::I16 => "movswq",
            IrType::U16 => "movzwq",
            IrType::I32 => "movslq",
            IrType::U32 | IrType::F32 => "movl",     // movl zero-extends to 64-bit implicitly
            _ => "movq",
        }
    }

    /// Destination register for loads. U32/F32 use movl which needs %eax.
    pub(super) fn load_dest_reg(ty: IrType) -> &'static str {
        match ty {
            IrType::U32 | IrType::F32 => "%eax",
            _ => "%rax",
        }
    }

    /// Map base register name + type to sized sub-register.
    pub(super) fn reg_for_type(base_reg: &str, ty: IrType) -> &'static str {
        let (r8, r16, r32, r64) = match base_reg {
            "rax" => ("al", "ax", "eax", "rax"),
            "rcx" => ("cl", "cx", "ecx", "rcx"),
            "rdx" => ("dl", "dx", "edx", "rdx"),
            "rdi" => ("dil", "di", "edi", "rdi"),
            "rsi" => ("sil", "si", "esi", "rsi"),
            "r8"  => ("r8b", "r8w", "r8d", "r8"),
            "r9"  => ("r9b", "r9w", "r9d", "r9"),
            _ => return "rax",
        };
        match ty {
            IrType::I8 | IrType::U8 => r8,
            IrType::I16 | IrType::U16 => r16,
            IrType::I32 | IrType::U32 | IrType::F32 => r32,
            _ => r64,
        }
    }

    /// Get the type suffix for lock-prefixed instructions (b, w, l, q).
    pub(super) fn type_suffix(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "b",
            IrType::I16 | IrType::U16 => "w",
            IrType::I32 | IrType::U32 => "l",
            _ => "q",
        }
    }

    /// Load an operand value into any GP register (returned as string).
    /// Uses rcx as the scratch register.
    pub(super) fn operand_to_reg(&mut self, op: &Operand, reg: &str) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) if *v == 0 => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", reg_name_to_32(reg))),
                    IrConst::I16(v) if *v == 0 => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", reg_name_to_32(reg))),
                    IrConst::I32(v) if *v == 0 => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", reg_name_to_32(reg))),
                    IrConst::I64(0) => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", reg_name_to_32(reg))),
                    IrConst::I8(v) => self.state.emit_fmt(format_args!("    movq ${}, %{}", *v as i64, reg)),
                    IrConst::I16(v) => self.state.emit_fmt(format_args!("    movq ${}, %{}", *v as i64, reg)),
                    IrConst::I32(v) => self.state.emit_fmt(format_args!("    movq ${}, %{}", *v as i64, reg)),
                    IrConst::I64(v) => {
                        if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", *v, reg);
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", *v, reg);
                        }
                    }
                    _ => self.state.emit_fmt(format_args!("    xorl %{0}, %{0}", reg_name_to_32(reg))),
                }
            }
            Operand::Value(v) => {
                self.value_to_reg(v, reg);
            }
        }
    }

    /// Extract an immediate integer value from an operand.
    /// Used for SSE/AES instructions that require compile-time immediate operands.
    pub(super) fn operand_to_imm_i64(&self, op: &Operand) -> i64 {
        match op {
            Operand::Const(c) => match c {
                IrConst::I8(v) => *v as i64,
                IrConst::I16(v) => *v as i64,
                IrConst::I32(v) => *v as i64,
                IrConst::I64(v) => *v,
                _ => 0,
            },
            Operand::Value(_) => {
                // TODO: this shouldn't happen for compile-time immediate arguments;
                // the frontend should always fold these to constants.
                0
            }
        }
    }

}
