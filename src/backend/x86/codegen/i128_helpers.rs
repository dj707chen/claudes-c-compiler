//! i128 operand loading, atomic operation loops, and variadic helpers for x86-64.
//!
//! 128-bit convention: %rax = low 64 bits, %rdx = high 64 bits.
//! Stack slots: offset(%rbp) = low, offset+8(%rbp) = high.

use crate::ir::ir::{IrConst, Operand, Value};
use crate::common::types::IrType;
use super::codegen::{X86Codegen, phys_reg_name};

impl X86Codegen {
    /// Load a 128-bit operand into %rax (low) and %rdx (high).
    pub(super) fn operand_to_rax_rdx(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I128(v) => {
                        let low = *v as u64 as i64;
                        let high = (*v >> 64) as u64 as i64;
                        if low == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else if low >= i32::MIN as i64 && low <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", low, "rax");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", low, "rax");
                        }
                        if high == 0 {
                            self.state.emit("    xorl %edx, %edx");
                        } else if high >= i32::MIN as i64 && high <= i32::MAX as i64 {
                            self.state.out.emit_instr_imm_reg("    movq", high, "rdx");
                        } else {
                            self.state.out.emit_instr_imm_reg("    movabsq", high, "rdx");
                        }
                    }
                    IrConst::Zero => {
                        self.state.emit("    xorl %eax, %eax");
                        self.state.emit("    xorl %edx, %edx");
                    }
                    _ => {
                        // Smaller constant: load into rax, zero/sign-extend to rdx
                        self.operand_to_rax(op);
                        self.state.emit("    xorl %edx, %edx");
                    }
                }
            }
            Operand::Value(v) => {
                if let Some(slot) = self.state.get_slot(v.0) {
                    if self.state.is_alloca(v.0) {
                        // Alloca: load the address (not a 128-bit value itself)
                        self.state.out.emit_instr_rbp_reg("    leaq", slot.0, "rax");
                        self.state.emit("    xorl %edx, %edx");
                    } else if self.state.is_i128_value(v.0) {
                        // 128-bit value in 16-byte stack slot
                        self.state.out.emit_instr_rbp_reg("    movq", slot.0, "rax");
                        self.state.out.emit_instr_rbp_reg("    movq", slot.0 + 8, "rdx");
                    } else {
                        // Non-i128 value (e.g. shift amount): load 8 bytes, zero-extend rdx
                        // Check register allocation first, since register-allocated values
                        // may not have their stack slot written.
                        if let Some(&reg) = self.reg_assignments.get(&v.0) {
                            let reg_name = phys_reg_name(reg);
                            self.state.out.emit_instr_reg_reg("    movq", reg_name, "rax");
                        } else {
                            self.state.out.emit_instr_rbp_reg("    movq", slot.0, "rax");
                        }
                        self.state.emit("    xorl %edx, %edx");
                    }
                } else {
                    // No stack slot: check register allocation
                    if let Some(&reg) = self.reg_assignments.get(&v.0) {
                        let reg_name = phys_reg_name(reg);
                        self.state.out.emit_instr_reg_reg("    movq", reg_name, "rax");
                        self.state.emit("    xorl %edx, %edx");
                    } else {
                        self.state.emit("    xorl %eax, %eax");
                        self.state.emit("    xorl %edx, %edx");
                    }
                }
            }
        }
    }

    /// Store %rax:%rdx (128-bit) to a value's 16-byte stack slot.
    pub(super) fn store_rax_rdx_to(&mut self, dest: &Value) {
        if let Some(slot) = self.state.get_slot(dest.0) {
            self.state.out.emit_instr_reg_rbp("    movq", "rax", slot.0);
            self.state.out.emit_instr_reg_rbp("    movq", "rdx", slot.0 + 8);
        }
        // rax holds only the low 64 bits of an i128, not a valid scalar IR value.
        self.state.reg_cache.invalidate_all();
    }

    /// Emit a cmpxchg-based loop for atomic sub/and/or/xor/nand.
    /// Expects: rax = operand val, rcx = ptr address.
    /// After: rax = old value.
    pub(super) fn emit_x86_atomic_op_loop(&mut self, ty: IrType, op: &str) {
        // Save val to r8
        self.state.emit("    movq %rax, %r8"); // r8 = val
        // Load old value
        let load_instr = Self::mov_load_for_type(ty);
        let load_dest = Self::load_dest_reg(ty);
        self.state.emit_fmt(format_args!("    {} (%rcx), {}", load_instr, load_dest));
        // Loop: rax = old, compute new = op(old, val), try cmpxchg
        let label_id = self.state.next_label_id();
        let loop_label = format!(".Latomic_loop_{}", label_id);
        self.state.out.emit_named_label(&loop_label);
        // rdx = rax (old)
        self.state.emit("    movq %rax, %rdx");
        // Apply operation: rdx = op(rdx, r8)
        let size_suffix = Self::type_suffix(ty);
        let rdx_reg = Self::reg_for_type("rdx", ty);
        let r8_reg = match ty {
            IrType::I8 | IrType::U8 => "r8b",
            IrType::I16 | IrType::U16 => "r8w",
            IrType::I32 | IrType::U32 => "r8d",
            _ => "r8",
        };
        match op {
            "sub" => self.state.emit_fmt(format_args!("    sub{} %{}, %{}", size_suffix, r8_reg, rdx_reg)),
            "and" => self.state.emit_fmt(format_args!("    and{} %{}, %{}", size_suffix, r8_reg, rdx_reg)),
            "or"  => self.state.emit_fmt(format_args!("    or{} %{}, %{}", size_suffix, r8_reg, rdx_reg)),
            "xor" => self.state.emit_fmt(format_args!("    xor{} %{}, %{}", size_suffix, r8_reg, rdx_reg)),
            "nand" => {
                self.state.emit_fmt(format_args!("    and{} %{}, %{}", size_suffix, r8_reg, rdx_reg));
                self.state.emit_fmt(format_args!("    not{} %{}", size_suffix, rdx_reg));
            }
            _ => {}
        }
        // Try cmpxchg: if [rcx] == rax (old), set [rcx] = rdx (new), else rax = [rcx]
        self.state.emit_fmt(format_args!("    lock cmpxchg{} %{}, (%rcx)", size_suffix, rdx_reg));
        self.state.out.emit_jcc_label("    jne", &loop_label);
        // rax = old value on success
    }

    /// Load i128 operands for binary ops: lhs → rax:rdx, rhs → rcx:rsi.
    pub(super) fn prep_i128_binop(&mut self, lhs: &Operand, rhs: &Operand) {
        self.operand_to_rax_rdx(lhs);
        self.state.emit("    pushq %rdx");
        self.state.emit("    pushq %rax");
        self.operand_to_rax_rdx(rhs);
        self.state.emit("    movq %rax, %rcx");
        self.state.emit("    movq %rdx, %rsi");
        self.state.emit("    popq %rax");
        self.state.emit("    popq %rdx");
        self.state.reg_cache.invalidate_all();
    }

    /// Helper to load va_list pointer into %rcx for va_arg operations.
    pub(super) fn load_va_list_ptr_to_rcx(&mut self, va_list_ptr: &Value) {
        if let Some(&reg) = self.reg_assignments.get(&va_list_ptr.0) {
            let reg_name = phys_reg_name(reg);
            self.state.out.emit_instr_reg_reg("    movq", reg_name, "rcx");
        } else if let Some(slot) = self.state.get_slot(va_list_ptr.0) {
            if self.state.is_alloca(va_list_ptr.0) {
                self.state.out.emit_instr_rbp_reg("    leaq", slot.0, "rcx");
            } else {
                self.state.out.emit_instr_rbp_reg("    movq", slot.0, "rcx");
            }
        }
    }

}
