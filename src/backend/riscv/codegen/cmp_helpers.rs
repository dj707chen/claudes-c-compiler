//! Comparison operand loading helpers for RISC-V 64.

use crate::ir::ir::Operand;
use crate::common::types::IrType;
use super::codegen::RiscvCodegen;

impl RiscvCodegen {
    /// Load comparison operands into t1 and t2, then sign/zero-extend
    /// sub-64-bit types. Shared by emit_cmp and emit_fused_cmp_branch.
    pub(super) fn emit_cmp_operand_load(&mut self, lhs: &Operand, rhs: &Operand, ty: IrType) {
        self.operand_to_t0(lhs);
        self.state.emit("    mv t1, t0");
        self.operand_to_t0(rhs);
        self.state.emit("    mv t2, t0");

        // Sign/zero-extend operands to 64 bits based on their actual type width.
        // The narrow optimization pass can produce I8/I16/U8/U16 typed comparisons,
        // so we must extend at the correct width, not just 32-bit for all sub-64 types.
        match ty {
            IrType::U8 => {
                self.state.emit("    andi t1, t1, 0xff");
                self.state.emit("    andi t2, t2, 0xff");
            }
            IrType::U16 => {
                self.state.emit("    slli t1, t1, 48");
                self.state.emit("    srli t1, t1, 48");
                self.state.emit("    slli t2, t2, 48");
                self.state.emit("    srli t2, t2, 48");
            }
            IrType::U32 => {
                self.state.emit("    slli t1, t1, 32");
                self.state.emit("    srli t1, t1, 32");
                self.state.emit("    slli t2, t2, 32");
                self.state.emit("    srli t2, t2, 32");
            }
            IrType::I8 => {
                self.state.emit("    slli t1, t1, 56");
                self.state.emit("    srai t1, t1, 56");
                self.state.emit("    slli t2, t2, 56");
                self.state.emit("    srai t2, t2, 56");
            }
            IrType::I16 => {
                self.state.emit("    slli t1, t1, 48");
                self.state.emit("    srai t1, t1, 48");
                self.state.emit("    slli t2, t2, 48");
                self.state.emit("    srai t2, t2, 48");
            }
            IrType::I32 => {
                self.state.emit("    sext.w t1, t1");
                self.state.emit("    sext.w t2, t2");
            }
            _ => {} // I64/U64/Ptr: no extension needed
        }
    }

}
