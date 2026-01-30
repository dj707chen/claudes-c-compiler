//! x87 FPU floating-point helpers for i686.
//!
//! F128 (long double) and F64 values are handled via the x87 FPU stack
//! on i686, since SSE support is limited to F32 operations.

use crate::ir::ir::{IrConst, Operand, Value};
use super::codegen::I686Codegen;
use crate::emit;

impl I686Codegen {
    /// Load an F128 (long double) operand onto the x87 FPU stack.
    pub(super) fn emit_f128_load_to_x87(&mut self, op: &Operand) {
        match op {
            Operand::Value(v) => {
                if let Some(slot) = self.state.get_slot(v.0) {
                    emit!(self.state, "    fldt {}(%ebp)", slot.0);
                }
            }
            Operand::Const(IrConst::LongDouble(_, bytes)) => {
                // Convert f128 (IEEE binary128) bytes to x87 80-bit format for fldt
                let x87 = crate::common::long_double::f128_bytes_to_x87_bytes(bytes);
                let dword0 = i32::from_le_bytes([x87[0], x87[1], x87[2], x87[3]]);
                let dword1 = i32::from_le_bytes([x87[4], x87[5], x87[6], x87[7]]);
                let word2 = i16::from_le_bytes([x87[8], x87[9]]) as i32;
                self.state.emit("    subl $12, %esp");
                emit!(self.state, "    movl ${}, (%esp)", dword0);
                emit!(self.state, "    movl ${}, 4(%esp)", dword1);
                emit!(self.state, "    movw ${}, 8(%esp)", word2);
                self.state.emit("    fldt (%esp)");
                self.state.emit("    addl $12, %esp");
            }
            Operand::Const(IrConst::F64(fval)) => {
                // Convert f64 to x87: push to stack as f64, fld, convert
                let bits = fval.to_bits();
                let low = (bits & 0xFFFFFFFF) as i32;
                let high = ((bits >> 32) & 0xFFFFFFFF) as i32;
                self.state.emit("    subl $8, %esp");
                emit!(self.state, "    movl ${}, (%esp)", low);
                emit!(self.state, "    movl ${}, 4(%esp)", high);
                self.state.emit("    fldl (%esp)");
                self.state.emit("    addl $8, %esp");
            }
            Operand::Const(IrConst::F32(fval)) => {
                emit!(self.state, "    movl ${}, %eax", fval.to_bits() as i32);
                self.state.emit("    pushl %eax");
                self.state.emit("    flds (%esp)");
                self.state.emit("    addl $4, %esp");
            }
            _ => {
                self.operand_to_eax(op);
                // Fallback: treat as integer, push to stack
                self.state.emit("    pushl %eax");
                self.state.emit("    flds (%esp)");
                self.state.emit("    addl $4, %esp");
            }
        }
    }

    /// Load an F64 (double) operand onto the x87 FPU stack.
    /// F64 values occupy 8-byte stack slots on i686, so we use fldl to load
    /// them directly from memory rather than going through the 32-bit accumulator.
    pub(super) fn emit_f64_load_to_x87(&mut self, op: &Operand) {
        match op {
            Operand::Value(v) => {
                if let Some(slot) = self.state.get_slot(v.0) {
                    emit!(self.state, "    fldl {}(%ebp)", slot.0);
                }
            }
            Operand::Const(IrConst::F64(fval)) => {
                let bits = fval.to_bits();
                let low = (bits & 0xFFFFFFFF) as i32;
                let high = ((bits >> 32) & 0xFFFFFFFF) as i32;
                self.state.emit("    subl $8, %esp");
                emit!(self.state, "    movl ${}, (%esp)", low);
                emit!(self.state, "    movl ${}, 4(%esp)", high);
                self.state.emit("    fldl (%esp)");
                self.state.emit("    addl $8, %esp");
            }
            Operand::Const(IrConst::F32(fval)) => {
                emit!(self.state, "    movl ${}, %eax", fval.to_bits() as i32);
                self.state.emit("    pushl %eax");
                self.state.emit("    flds (%esp)");
                self.state.emit("    addl $4, %esp");
            }
            Operand::Const(IrConst::Zero) => {
                self.state.emit("    fldz");
            }
            _ => {
                // Fallback: load integer bits and convert
                self.operand_to_eax(op);
                self.state.emit("    pushl %eax");
                self.state.emit("    fildl (%esp)");
                self.state.emit("    addl $4, %esp");
            }
        }
    }

    /// Store the x87 st(0) value as F64 into a destination stack slot.
    /// Pops st(0).
    pub(super) fn emit_f64_store_from_x87(&mut self, dest: &Value) {
        if let Some(slot) = self.state.get_slot(dest.0) {
            emit!(self.state, "    fstpl {}(%ebp)", slot.0);
        } else {
            // No slot available, pop x87 stack to discard
            self.state.emit("    fstp %st(0)");
        }
    }

}
