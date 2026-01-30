//! Integer comparison instruction emission for AArch64.

use crate::ir::ir::Operand;
use crate::common::types::IrType;
use super::codegen::ArmCodegen;

impl ArmCodegen {
    /// Emit the integer comparison preamble.
    /// Optimized paths:
    ///   1. reg vs #imm12 → `cmp wN/xN, #imm` (1 instruction)
    ///   2. reg vs #neg_imm12 → `cmn wN/xN, #imm` (1 instruction)
    ///   3. reg vs reg → `cmp wN/xN, wM/xM` (1 instruction)
    ///   4. fallback → load lhs→x1, rhs→x0, `cmp w1/x1, w0/x0`
    ///      Used by both emit_cmp and emit_fused_cmp_branch.
    pub(super) fn emit_int_cmp_insn(&mut self, lhs: &Operand, rhs: &Operand, ty: IrType) {
        let use_32bit = ty == IrType::I32 || ty == IrType::U32
            || ty == IrType::I8 || ty == IrType::U8
            || ty == IrType::I16 || ty == IrType::U16;

        // Try optimized path: lhs in register, rhs is immediate
        if let Operand::Value(lv) = lhs {
            if let Some((lhs_x, lhs_w)) = self.value_reg_name(lv) {
                let lhs_reg = if use_32bit { lhs_w } else { lhs_x };

                // cmp reg, #imm12
                if let Operand::Const(c) = rhs {
                    if let Some(imm) = Self::const_as_cmp_imm12(c) {
                        self.state.emit_fmt(format_args!("    cmp {}, #{}", lhs_reg, imm));
                        return;
                    }
                    // cmn reg, #imm12 (for negative constants)
                    if let Some(imm) = Self::const_as_cmn_imm12(c) {
                        self.state.emit_fmt(format_args!("    cmn {}, #{}", lhs_reg, imm));
                        return;
                    }
                }

                // cmp reg, reg
                if let Operand::Value(rv) = rhs {
                    if let Some((rhs_x, rhs_w)) = self.value_reg_name(rv) {
                        let rhs_reg = if use_32bit { rhs_w } else { rhs_x };
                        self.state.emit_fmt(format_args!("    cmp {}, {}", lhs_reg, rhs_reg));
                        return;
                    }
                }

                // lhs in register, rhs needs loading into x0
                self.operand_to_x0(rhs);
                if use_32bit {
                    self.state.emit_fmt(format_args!("    cmp {}, w0", lhs_reg));
                } else {
                    self.state.emit_fmt(format_args!("    cmp {}, x0", lhs_reg));
                }
                return;
            }
        }

        // Try: lhs needs loading, rhs in register
        if let Operand::Value(rv) = rhs {
            if let Some((rhs_x, rhs_w)) = self.value_reg_name(rv) {
                self.operand_to_x0(lhs);
                let rhs_reg = if use_32bit { rhs_w } else { rhs_x };
                if use_32bit {
                    self.state.emit_fmt(format_args!("    cmp w0, {}", rhs_reg));
                } else {
                    self.state.emit_fmt(format_args!("    cmp x0, {}", rhs_reg));
                }
                return;
            }
        }

        // Try: lhs in x0 (accumulator), rhs is immediate
        if let Operand::Const(c) = rhs {
            if let Some(imm) = Self::const_as_cmp_imm12(c) {
                self.operand_to_x0(lhs);
                if use_32bit {
                    self.state.emit_fmt(format_args!("    cmp w0, #{}", imm));
                } else {
                    self.state.emit_fmt(format_args!("    cmp x0, #{}", imm));
                }
                return;
            }
            if let Some(imm) = Self::const_as_cmn_imm12(c) {
                self.operand_to_x0(lhs);
                if use_32bit {
                    self.state.emit_fmt(format_args!("    cmn w0, #{}", imm));
                } else {
                    self.state.emit_fmt(format_args!("    cmn x0, #{}", imm));
                }
                return;
            }
        }

        // Fallback: load both into x0/x1
        self.operand_to_x0(lhs);
        self.state.emit("    mov x1, x0");
        self.operand_to_x0(rhs);
        if use_32bit {
            self.state.emit("    cmp w1, w0");
        } else {
            self.state.emit("    cmp x1, x0");
        }
    }

}
