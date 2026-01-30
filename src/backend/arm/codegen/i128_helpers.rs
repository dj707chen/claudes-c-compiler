//! i128 operand loading helpers for AArch64.
//!
//! 128-bit convention: x0 = low 64 bits, x1 = high 64 bits.

use crate::ir::ir::{IrConst, Operand, Value};
use super::codegen::{ArmCodegen, callee_saved_name};

impl ArmCodegen {
    /// Load a 128-bit operand into x0 (low) : x1 (high).
    pub(super) fn operand_to_x0_x1(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I128(v) => {
                        let low = *v as u64;
                        let high = (*v >> 64) as u64;
                        self.emit_load_imm64("x0", low as i64);
                        self.emit_load_imm64("x1", high as i64);
                    }
                    IrConst::Zero => {
                        self.state.emit("    mov x0, #0");
                        self.state.emit("    mov x1, #0");
                    }
                    _ => {
                        // Other consts: load into x0, zero-extend high half
                        self.operand_to_x0(op);
                        self.state.emit("    mov x1, #0");
                    }
                }
            }
            Operand::Value(v) => {
                if let Some(slot) = self.state.get_slot(v.0) {
                    if self.state.is_alloca(v.0) {
                        // Alloca: address, not a 128-bit value itself
                        self.emit_add_sp_offset("x0", slot.0);
                        self.state.emit("    mov x1, #0");
                    } else if self.state.is_i128_value(v.0) {
                        // 128-bit value in 16-byte stack slot
                        self.emit_load_from_sp("x0", slot.0, "ldr");
                        self.emit_load_from_sp("x1", slot.0 + 8, "ldr");
                    } else {
                        // Non-i128 value (e.g. shift amount): load 8 bytes, zero high
                        // Check register allocation first, since register-allocated values
                        // may not have their stack slot written.
                        if let Some(&reg) = self.reg_assignments.get(&v.0) {
                            let reg_name = callee_saved_name(reg);
                            self.state.emit_fmt(format_args!("    mov x0, {}", reg_name));
                        } else {
                            self.emit_load_from_sp("x0", slot.0, "ldr");
                        }
                        self.state.emit("    mov x1, #0");
                    }
                } else {
                    // No stack slot: check register allocation
                    if let Some(&reg) = self.reg_assignments.get(&v.0) {
                        let reg_name = callee_saved_name(reg);
                        self.state.emit_fmt(format_args!("    mov x0, {}", reg_name));
                        self.state.emit("    mov x1, #0");
                    } else {
                        self.state.emit("    mov x0, #0");
                        self.state.emit("    mov x1, #0");
                    }
                }
            }
        }
    }

    /// Store x0 (low) : x1 (high) to a 128-bit value's stack slot.
    pub(super) fn store_x0_x1_to(&mut self, dest: &Value) {
        if let Some(slot) = self.state.get_slot(dest.0) {
            self.emit_store_to_sp("x0", slot.0, "str");
            self.emit_store_to_sp("x1", slot.0 + 8, "str");
        }
    }

    /// Prepare a 128-bit binary operation: load lhs into x2:x3, rhs into x4:x5.
    /// (Uses x0:x1 as temporaries during loading.)
    pub(super) fn prep_i128_binop(&mut self, lhs: &Operand, rhs: &Operand) {
        self.operand_to_x0_x1(lhs);
        self.state.emit("    mov x2, x0");
        self.state.emit("    mov x3, x1");
        self.operand_to_x0_x1(rhs);
        self.state.emit("    mov x4, x0");
        self.state.emit("    mov x5, x1");
    }

}
