//! Integer ALU and comparison helper methods for x86-64.
//!
//! Register-direct paths for ALU operations, shift operations, and
//! accumulator-based immediate optimizations. These are called by the
//! trait methods `emit_int_binop` and `emit_int_cmp`.

use crate::ir::ir::{IrBinOp, Operand, Value};
use crate::backend::regalloc::PhysReg;
use super::codegen::{X86Codegen, phys_reg_name, phys_reg_name_32, alu_mnemonic, shift_mnemonic};

impl X86Codegen {
    /// Emit a comparison instruction, optionally using 32-bit form for I32/U32 types.
    /// When `use_32bit` is true, emits `cmpl` with 32-bit register names instead of `cmpq`.
    pub(super) fn emit_int_cmp_insn_typed(&mut self, lhs: &Operand, rhs: &Operand, use_32bit: bool) {
        let cmp_instr = if use_32bit { "cmpl" } else { "cmpq" };
        let lhs_phys = self.operand_reg(lhs);
        let rhs_phys = self.operand_reg(rhs);
        if let (Some(lhs_r), Some(rhs_r)) = (lhs_phys, rhs_phys) {
            // Both in callee-saved registers: compare directly
            let lhs_name = if use_32bit { phys_reg_name_32(lhs_r) } else { phys_reg_name(lhs_r) };
            let rhs_name = if use_32bit { phys_reg_name_32(rhs_r) } else { phys_reg_name(rhs_r) };
            self.state.emit_fmt(format_args!("    {} %{}, %{}", cmp_instr, rhs_name, lhs_name));
        } else if let Some(imm) = Self::const_as_imm32(rhs) {
            if imm == 0 {
                // test %reg, %reg is shorter than cmp $0, %reg and sets flags identically
                let test_instr = if use_32bit { "testl" } else { "testq" };
                if let Some(lhs_r) = lhs_phys {
                    let lhs_name = if use_32bit { phys_reg_name_32(lhs_r) } else { phys_reg_name(lhs_r) };
                    self.state.emit_fmt(format_args!("    {} %{}, %{}", test_instr, lhs_name, lhs_name));
                } else {
                    self.operand_to_rax(lhs);
                    let reg = if use_32bit { "eax" } else { "rax" };
                    self.state.emit_fmt(format_args!("    {} %{}, %{}", test_instr, reg, reg));
                }
            } else if let Some(lhs_r) = lhs_phys {
                let lhs_name = if use_32bit { phys_reg_name_32(lhs_r) } else { phys_reg_name(lhs_r) };
                self.state.emit_fmt(format_args!("    {} ${}, %{}", cmp_instr, imm, lhs_name));
            } else {
                self.operand_to_rax(lhs);
                let reg = if use_32bit { "eax" } else { "rax" };
                self.state.emit_fmt(format_args!("    {} ${}, %{}", cmp_instr, imm, reg));
            }
        } else if let Some(lhs_r) = lhs_phys {
            let lhs_name = if use_32bit { phys_reg_name_32(lhs_r) } else { phys_reg_name(lhs_r) };
            self.operand_to_rcx(rhs);
            let rcx = if use_32bit { "ecx" } else { "rcx" };
            self.state.emit_fmt(format_args!("    {} %{}, %{}", cmp_instr, rcx, lhs_name));
        } else if let Some(rhs_r) = rhs_phys {
            let rhs_name = if use_32bit { phys_reg_name_32(rhs_r) } else { phys_reg_name(rhs_r) };
            self.operand_to_rax(lhs);
            let reg = if use_32bit { "eax" } else { "rax" };
            self.state.emit_fmt(format_args!("    {} %{}, %{}", cmp_instr, rhs_name, reg));
        } else {
            self.operand_to_rax(lhs);
            self.operand_to_rcx(rhs);
            let (rcx, rax) = if use_32bit { ("ecx", "eax") } else { ("rcx", "rax") };
            self.state.emit_fmt(format_args!("    {} %{}, %{}", cmp_instr, rcx, rax));
        }
    }

    /// Emit comment annotations for callee-saved registers listed in inline asm
    /// clobber lists. The peephole pass's `eliminate_unused_callee_saves` scans
    /// function bodies for textual register references (e.g., "%rbx") to decide
    /// whether a callee-saved register save/restore can be eliminated. Without
    /// these annotations, an inline asm that clobbers a callee-saved register
    /// (but doesn't mention it in the emitted assembly text) would have its
    /// save/restore incorrectly removed.
    pub(super) fn emit_callee_saved_clobber_annotations(&mut self, clobbers: &[String]) {
        for clobber in clobbers {
            let reg_name = match clobber.as_str() {
                "rbx" | "ebx" | "bx" | "bl" | "bh" => Some("%rbx"),
                "r12" | "r12d" | "r12w" | "r12b" => Some("%r12"),
                "r13" | "r13d" | "r13w" | "r13b" => Some("%r13"),
                "r14" | "r14d" | "r14w" | "r14b" => Some("%r14"),
                "r15" | "r15d" | "r15w" | "r15b" => Some("%r15"),
                _ => None,
            };
            if let Some(reg) = reg_name {
                self.state.emit_fmt(format_args!("    # asm clobber {}", reg));
            }
        }
    }

    /// LEA scale factor for multiply strength reduction.
    /// Returns the LEA scale factor for multipliers 3, 5, 9 (which decompose
    /// as reg + reg*2, reg + reg*4, reg + reg*8 respectively).
    pub(super) fn lea_scale_for_mul(imm: i64) -> Option<u8> {
        match imm {
            3 => Some(2),
            5 => Some(4),
            9 => Some(8),
            _ => None,
        }
    }

    /// Register-direct path for simple ALU ops (add/sub/and/or/xor/mul).
    pub(super) fn emit_alu_reg_direct(&mut self, op: IrBinOp, lhs: &Operand, rhs: &Operand,
                           dest_phys: PhysReg, use_32bit: bool, is_unsigned: bool) {
        let dest_name = phys_reg_name(dest_phys);
        let dest_name_32 = phys_reg_name_32(dest_phys);

        // Immediate form
        if let Some(imm) = Self::const_as_imm32(rhs) {
            self.operand_to_callee_reg(lhs, dest_phys);
            if op == IrBinOp::Mul {
                // LEA strength reduction: replace imul by 3/5/9 with lea.
                // lea (%reg, %reg, scale), %reg computes reg + reg*scale = reg*(scale+1).
                // lea has 1-cycle latency vs 3 cycles for imul on modern x86.
                if let Some(scale) = Self::lea_scale_for_mul(imm) {
                    if use_32bit {
                        self.state.emit_fmt(format_args!(
                            "    leal (%{}, %{}, {}), %{}", dest_name_32, dest_name_32, scale, dest_name_32));
                        self.emit_sext32_if_needed(dest_name_32, dest_name, is_unsigned);
                    } else {
                        self.state.emit_fmt(format_args!(
                            "    leaq (%{}, %{}, {}), %{}", dest_name, dest_name, scale, dest_name));
                    }
                } else if use_32bit {
                    self.state.emit_fmt(format_args!("    imull ${}, %{}, %{}", imm, dest_name_32, dest_name_32));
                    self.emit_sext32_if_needed(dest_name_32, dest_name, is_unsigned);
                } else {
                    self.state.emit_fmt(format_args!("    imulq ${}, %{}, %{}", imm, dest_name, dest_name));
                }
            } else {
                let mnemonic = alu_mnemonic(op);
                if use_32bit && matches!(op, IrBinOp::Add | IrBinOp::Sub) {
                    self.state.emit_fmt(format_args!("    {}l ${}, %{}", mnemonic, imm, dest_name_32));
                    self.emit_sext32_if_needed(dest_name_32, dest_name, is_unsigned);
                } else {
                    self.state.emit_fmt(format_args!("    {}q ${}, %{}", mnemonic, imm, dest_name));
                }
            }
            self.state.reg_cache.invalidate_acc();
            return;
        }

        // Register-register form
        let rhs_phys = self.operand_reg(rhs);
        let rhs_conflicts = rhs_phys.is_some_and(|r| r.0 == dest_phys.0);
        let (rhs_reg_name, rhs_reg_name_32): (String, String) = if rhs_conflicts {
            self.operand_to_rax(rhs);
            self.operand_to_callee_reg(lhs, dest_phys);
            ("rax".to_string(), "eax".to_string())
        } else {
            self.operand_to_callee_reg(lhs, dest_phys);
            if let Some(rhs_phys) = rhs_phys {
                (phys_reg_name(rhs_phys).to_string(), phys_reg_name_32(rhs_phys).to_string())
            } else {
                self.operand_to_rax(rhs);
                ("rax".to_string(), "eax".to_string())
            }
        };

        if op == IrBinOp::Mul {
            if use_32bit {
                self.state.out.emit_instr_reg_reg("    imull", &rhs_reg_name_32, dest_name_32);
                self.emit_sext32_if_needed(dest_name_32, dest_name, is_unsigned);
            } else {
                self.state.out.emit_instr_reg_reg("    imulq", &rhs_reg_name, dest_name);
            }
        } else {
            let mnemonic = alu_mnemonic(op);
            if use_32bit && matches!(op, IrBinOp::Add | IrBinOp::Sub) {
                self.state.emit_fmt(format_args!("    {}l %{}, %{}", mnemonic, rhs_reg_name_32, dest_name_32));
                self.emit_sext32_if_needed(dest_name_32, dest_name, is_unsigned);
            } else {
                self.state.emit_fmt(format_args!("    {}q %{}, %{}", mnemonic, rhs_reg_name, dest_name));
            }
        }
        self.state.reg_cache.invalidate_acc();
    }

    /// Register-direct path for shift operations.
    pub(super) fn emit_shift_reg_direct(&mut self, op: IrBinOp, lhs: &Operand, rhs: &Operand,
                             dest_phys: PhysReg, use_32bit: bool, is_unsigned: bool) {
        let dest_name = phys_reg_name(dest_phys);
        let dest_name_32 = phys_reg_name_32(dest_phys);
        let (mnem32, mnem64) = shift_mnemonic(op);

        if let Some(imm) = Self::const_as_imm32(rhs) {
            self.operand_to_callee_reg(lhs, dest_phys);
            if use_32bit {
                let shift_amount = (imm as u32) & 31;
                self.state.emit_fmt(format_args!("    {} ${}, %{}", mnem32, shift_amount, dest_name_32));
                if !is_unsigned && matches!(op, IrBinOp::Shl | IrBinOp::AShr) {
                    self.state.out.emit_instr_reg_reg("    movslq", dest_name_32, dest_name);
                }
            } else {
                let shift_amount = (imm as u64) & 63;
                self.state.emit_fmt(format_args!("    {} ${}, %{}", mnem64, shift_amount, dest_name));
            }
        } else {
            let rhs_conflicts = self.operand_reg(rhs).is_some_and(|r| r.0 == dest_phys.0);
            if rhs_conflicts {
                self.operand_to_rcx(rhs);
                self.operand_to_callee_reg(lhs, dest_phys);
            } else {
                self.operand_to_callee_reg(lhs, dest_phys);
                self.operand_to_rcx(rhs);
            }
            if use_32bit {
                self.state.emit_fmt(format_args!("    {} %cl, %{}", mnem32, dest_name_32));
                if !is_unsigned && matches!(op, IrBinOp::Shl | IrBinOp::AShr) {
                    self.state.out.emit_instr_reg_reg("    movslq", dest_name_32, dest_name);
                }
            } else {
                self.state.emit_fmt(format_args!("    {} %cl, %{}", mnem64, dest_name));
            }
        }
        self.state.reg_cache.invalidate_acc();
    }

    /// Accumulator-based path: try immediate optimizations first.
    /// Returns true if handled.
    pub(super) fn try_emit_acc_immediate(&mut self, dest: &Value, op: IrBinOp, lhs: &Operand, rhs: &Operand,
                              use_32bit: bool, is_unsigned: bool) -> bool {
        // Immediate ALU ops
        if matches!(op, IrBinOp::Add | IrBinOp::Sub | IrBinOp::And | IrBinOp::Or | IrBinOp::Xor) {
            if let Some(imm) = Self::const_as_imm32(rhs) {
                self.operand_to_rax(lhs);
                let mnemonic = alu_mnemonic(op);
                if use_32bit && matches!(op, IrBinOp::Add | IrBinOp::Sub) {
                    self.state.emit_fmt(format_args!("    {}l ${}, %eax", mnemonic, imm));
                    if !is_unsigned { self.state.emit("    cltq"); }
                } else {
                    self.state.emit_fmt(format_args!("    {}q ${}, %rax", mnemonic, imm));
                }
                self.state.reg_cache.invalidate_acc();
                self.store_rax_to(dest);
                return true;
            }
        }

        // Immediate multiply
        if op == IrBinOp::Mul {
            if let Some(imm) = Self::const_as_imm32(rhs) {
                self.operand_to_rax(lhs);
                // LEA strength reduction: x*3/5/9 → lea (%rax, %rax, scale), %rax.
                // lea has 1-cycle latency vs 3 cycles for imul on modern x86.
                if let Some(scale) = Self::lea_scale_for_mul(imm) {
                    if use_32bit {
                        self.state.emit_fmt(format_args!("    leal (%eax, %eax, {}), %eax", scale));
                        if !is_unsigned { self.state.emit("    cltq"); }
                    } else {
                        self.state.emit_fmt(format_args!("    leaq (%rax, %rax, {}), %rax", scale));
                    }
                } else if use_32bit {
                    self.state.emit_fmt(format_args!("    imull ${}, %eax, %eax", imm));
                    if !is_unsigned { self.state.emit("    cltq"); }
                } else {
                    self.state.emit_fmt(format_args!("    imulq ${}, %rax, %rax", imm));
                }
                self.state.reg_cache.invalidate_acc();
                self.store_rax_to(dest);
                return true;
            }
        }

        // Immediate shift
        if matches!(op, IrBinOp::Shl | IrBinOp::AShr | IrBinOp::LShr) {
            if let Some(imm) = Self::const_as_imm32(rhs) {
                self.operand_to_rax(lhs);
                let (mnem32, mnem64) = shift_mnemonic(op);
                if use_32bit {
                    let shift_amount = (imm as u32) & 31;
                    self.state.emit_fmt(format_args!("    {} ${}, %eax", mnem32, shift_amount));
                    if !is_unsigned && matches!(op, IrBinOp::Shl | IrBinOp::AShr) {
                        self.state.emit("    cltq");
                    }
                } else {
                    let shift_amount = (imm as u64) & 63;
                    self.state.emit_fmt(format_args!("    {} ${}, %rax", mnem64, shift_amount));
                }
                self.state.reg_cache.invalidate_acc();
                self.store_rax_to(dest);
                return true;
            }
        }

        false
    }

}
