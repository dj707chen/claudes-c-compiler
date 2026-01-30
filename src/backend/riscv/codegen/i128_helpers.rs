//! i128 operand loading helpers for RISC-V 64.
//!
//! 128-bit convention: t0 = low 64 bits, t1 = high 64 bits.

use crate::ir::ir::{IrConst, Operand, Value};
use super::codegen::{RiscvCodegen, callee_saved_name};

impl RiscvCodegen {
    /// Load a 128-bit operand into t0 (low) : t1 (high).
    pub(super) fn operand_to_t0_t1(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I128(v) => {
                        let low = *v as u64 as i64;
                        let high = (*v >> 64) as u64 as i64;
                        self.state.emit_fmt(format_args!("    li t0, {}", low));
                        self.state.emit_fmt(format_args!("    li t1, {}", high));
                    }
                    IrConst::Zero => {
                        self.state.emit("    li t0, 0");
                        self.state.emit("    li t1, 0");
                    }
                    _ => {
                        self.operand_to_t0(op);
                        self.state.emit("    li t1, 0");
                    }
                }
            }
            Operand::Value(v) => {
                if let Some(slot) = self.state.get_slot(v.0) {
                    if self.state.is_alloca(v.0) {
                        self.emit_addi_s0("t0", slot.0);
                        self.state.emit("    li t1, 0");
                    } else if self.state.is_i128_value(v.0) {
                        // 128-bit value in 16-byte stack slot
                        self.emit_load_from_s0("t0", slot.0, "ld");
                        self.emit_load_from_s0("t1", slot.0 + 8, "ld");
                    } else {
                        // Non-i128 value (e.g. shift amount): load 8 bytes, zero high
                        // Check register allocation first, since register-allocated values
                        // may not have their stack slot written.
                        if let Some(&reg) = self.reg_assignments.get(&v.0) {
                            let reg_name = callee_saved_name(reg);
                            self.state.emit_fmt(format_args!("    mv t0, {}", reg_name));
                        } else {
                            self.emit_load_from_s0("t0", slot.0, "ld");
                        }
                        self.state.emit("    li t1, 0");
                    }
                } else {
                    // No stack slot: check register allocation
                    if let Some(&reg) = self.reg_assignments.get(&v.0) {
                        let reg_name = callee_saved_name(reg);
                        self.state.emit_fmt(format_args!("    mv t0, {}", reg_name));
                        self.state.emit("    li t1, 0");
                    } else {
                        self.state.emit("    li t0, 0");
                        self.state.emit("    li t1, 0");
                    }
                }
            }
        }
    }

    /// Store t0 (low) : t1 (high) to a 128-bit value's stack slot.
    pub(super) fn store_t0_t1_to(&mut self, dest: &Value) {
        if let Some(slot) = self.state.get_slot(dest.0) {
            self.emit_store_to_s0("t0", slot.0, "sd");
            self.emit_store_to_s0("t1", slot.0 + 8, "sd");
        }
    }

    /// Prepare a 128-bit binary operation: load lhs into t3:t4, rhs into t5:t6.
    pub(super) fn prep_i128_binop(&mut self, lhs: &Operand, rhs: &Operand) {
        self.operand_to_t0_t1(lhs);
        self.state.emit("    mv t3, t0");
        self.state.emit("    mv t4, t1");
        self.operand_to_t0_t1(rhs);
        self.state.emit("    mv t5, t0");
        self.state.emit("    mv t6, t1");
    }

}
