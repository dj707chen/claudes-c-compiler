//! Core operand loading and register manipulation helpers for AArch64.
//!
//! Handles loading IR operands into x0 (accumulator), storing results,
//! immediate encoding checks (imm12), and type-to-instruction mapping.

use crate::ir::ir::{IrConst, Operand, Value};
use crate::common::types::IrType;
use crate::backend::regalloc::PhysReg;
use super::codegen::{ArmCodegen, callee_saved_name, callee_saved_name_32};

impl ArmCodegen {
    /// Get the physical register assigned to an operand (if it's a Value with a register).
    pub(super) fn operand_reg(&self, op: &Operand) -> Option<PhysReg> {
        match op {
            Operand::Value(v) => self.reg_assignments.get(&v.0).copied(),
            _ => None,
        }
    }

    /// Get the physical register assigned to a destination value.
    pub(super) fn dest_reg(&self, dest: &Value) -> Option<PhysReg> {
        self.reg_assignments.get(&dest.0).copied()
    }

    /// Load an operand into a specific callee-saved register.
    pub(super) fn operand_to_callee_reg(&mut self, op: &Operand, reg: PhysReg) {
        let reg_name = callee_saved_name(reg);
        match op {
            Operand::Const(_) => {
                self.operand_to_x0(op);
                self.state.emit_fmt(format_args!("    mov {}, x0", reg_name));
            }
            Operand::Value(v) => {
                if let Some(&src_reg) = self.reg_assignments.get(&v.0) {
                    if src_reg.0 != reg.0 {
                        let src_name = callee_saved_name(src_reg);
                        self.state.emit_fmt(format_args!("    mov {}, {}", reg_name, src_name));
                    }
                } else {
                    self.operand_to_x0(op);
                    self.state.emit_fmt(format_args!("    mov {}, x0", reg_name));
                }
            }
        }
    }

    /// Try to extract an immediate value suitable for ARM imm12 encoding.
    pub(super) fn const_as_imm12(op: &Operand) -> Option<i64> {
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
                // ARM add/sub imm12: 0..4095
                if (0..=4095).contains(&val) {
                    Some(val)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// If `op` is a constant that is a power of two, return its log2 (shift amount).
    pub(super) fn const_as_power_of_2(op: &Operand) -> Option<u32> {
        match op {
            Operand::Const(c) => {
                let val: u64 = match c {
                    IrConst::I8(v) => *v as u8 as u64,
                    IrConst::I16(v) => *v as u16 as u64,
                    IrConst::I32(v) => *v as u32 as u64,
                    IrConst::I64(v) => *v as u64,
                    IrConst::Zero => return None,
                    _ => return None,
                };
                if val > 0 && val.is_power_of_two() {
                    Some(val.trailing_zeros())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check if an IrConst is a small unsigned immediate that fits in AArch64
    /// `cmp Xn, #imm12` instruction (0..=4095).
    pub(super) fn const_as_cmp_imm12(c: &IrConst) -> Option<u64> {
        let v = match c {
            IrConst::I8(v) => *v as i64,
            IrConst::I16(v) => *v as i64,
            IrConst::I32(v) => *v as i64,
            IrConst::I64(v) => *v,
            IrConst::Zero => 0,
            _ => return None,
        };
        // AArch64 cmp (alias of subs) accepts unsigned 12-bit immediate (0..4095),
        // optionally shifted left by 12. We only use the unshifted form.
        if (0..=4095).contains(&v) {
            Some(v as u64)
        } else {
            None
        }
    }

    /// Check if an IrConst is a small negative value that can use `cmn Xn, #imm12`
    /// (i.e., the negated value fits in 0..=4095).
    pub(super) fn const_as_cmn_imm12(c: &IrConst) -> Option<u64> {
        let v = match c {
            IrConst::I8(v) => *v as i64,
            IrConst::I16(v) => *v as i64,
            IrConst::I32(v) => *v as i64,
            IrConst::I64(v) => *v,
            _ => return None,
        };
        if v < 0 && (-v) >= 1 && (-v) <= 4095 {
            Some((-v) as u64)
        } else {
            None
        }
    }

    /// Get the register name for a Value if it has a register assignment.
    /// Returns (64-bit name, 32-bit name) pair.
    pub(super) fn value_reg_name(&self, v: &Value) -> Option<(&'static str, &'static str)> {
        self.reg_assignments.get(&v.0).map(|&reg| {
            (callee_saved_name(reg), callee_saved_name_32(reg))
        })
    }

    /// Load an operand into x0.
    pub(super) fn operand_to_x0(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                self.state.reg_cache.invalidate_acc();
                match c {
                    IrConst::I8(v) => self.state.emit_fmt(format_args!("    mov x0, #{}", v)),
                    IrConst::I16(v) => self.state.emit_fmt(format_args!("    mov x0, #{}", v)),
                    IrConst::I32(v) => {
                        if *v >= 0 && *v <= 65535 {
                            self.state.emit_fmt(format_args!("    mov x0, #{}", v));
                        } else if *v < 0 && *v >= -65536 {
                            self.state.emit_fmt(format_args!("    mov x0, #{}", v));
                        } else {
                            // Sign-extend to 64-bit before loading into x0.
                            // Using the i64 path ensures negative I32 values get
                            // proper sign extension (upper 32 bits = 0xFFFFFFFF).
                            self.emit_load_imm64("x0", *v as i64);
                        }
                    }
                    IrConst::I64(v) => {
                        if *v >= 0 && *v <= 65535 {
                            self.state.emit_fmt(format_args!("    mov x0, #{}", v));
                        } else if *v < 0 && *v >= -65536 {
                            self.state.emit_fmt(format_args!("    mov x0, #{}", v));
                        } else {
                            self.emit_load_imm64("x0", *v);
                        }
                    }
                    IrConst::F32(v) => self.emit_load_imm64("x0", v.to_bits() as i64),
                    IrConst::F64(v) => self.emit_load_imm64("x0", v.to_bits() as i64),
                    IrConst::LongDouble(v, _) => self.emit_load_imm64("x0", v.to_bits() as i64),
                    IrConst::I128(v) => self.emit_load_imm64("x0", *v as i64), // truncate to 64-bit
                    IrConst::Zero => self.state.emit("    mov x0, #0"),
                }
            }
            Operand::Value(v) => {
                let is_alloca = self.state.is_alloca(v.0);
                if self.state.reg_cache.acc_has(v.0, is_alloca) {
                    return; // Cache hit — x0 already holds this value.
                }
                // Check for callee-saved register assignment.
                if let Some(&reg) = self.reg_assignments.get(&v.0) {
                    let reg_name = callee_saved_name(reg);
                    self.state.emit_fmt(format_args!("    mov x0, {}", reg_name));
                    self.state.reg_cache.set_acc(v.0, false);
                    return;
                }
                if let Some(slot) = self.state.get_slot(v.0) {
                    if is_alloca {
                        if let Some(align) = self.state.alloca_over_align(v.0) {
                            // Over-aligned alloca: compute aligned address.
                            // x0 = (slot_addr + align-1) & -align
                            self.emit_add_sp_offset("x0", slot.0);
                            self.load_large_imm("x17", (align - 1) as i64);
                            self.state.emit("    add x0, x0, x17");
                            self.load_large_imm("x17", -(align as i64));
                            self.state.emit("    and x0, x0, x17");
                        } else {
                            self.emit_add_sp_offset("x0", slot.0);
                        }
                    } else {
                        self.emit_load_from_sp("x0", slot.0, "ldr");
                    }
                    self.state.reg_cache.set_acc(v.0, is_alloca);
                } else if self.state.reg_cache.acc_has(v.0, false) || self.state.reg_cache.acc_has(v.0, true) {
                    // Value has no slot or register but is in the accumulator cache
                    // (skip-slot optimization: immediately-consumed values stay in x0).
                    return;
                } else {
                    self.state.emit("    mov x0, #0");
                    self.state.reg_cache.invalidate_acc();
                }
            }
        }
    }

    /// Store x0 to a value's destination (register or stack slot).
    pub(super) fn store_x0_to(&mut self, dest: &Value) {
        if let Some(&reg) = self.reg_assignments.get(&dest.0) {
            // Value has a callee-saved register: store only to register, skip stack.
            let reg_name = callee_saved_name(reg);
            self.state.emit_fmt(format_args!("    mov {}, x0", reg_name));
        } else if let Some(slot) = self.state.get_slot(dest.0) {
            self.emit_store_to_sp("x0", slot.0, "str");
        }
        self.state.reg_cache.set_acc(dest.0, false);
    }

    pub(super) fn str_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "strb",
            IrType::I16 | IrType::U16 => "strh",
            IrType::I32 | IrType::U32 | IrType::F32 => "str",  // 32-bit store with w register
            _ => "str",  // 64-bit store with x register
        }
    }

    /// Get the appropriate register name for a given base and type.
    pub(super) fn reg_for_type(base: &str, ty: IrType) -> &'static str {
        let use_w = matches!(ty,
            IrType::I8 | IrType::U8 | IrType::I16 | IrType::U16 |
            IrType::I32 | IrType::U32 | IrType::F32
        );
        match base {
            "x0" => if use_w { "w0" } else { "x0" },
            "x1" => if use_w { "w1" } else { "x1" },
            "x2" => if use_w { "w2" } else { "x2" },
            "x3" => if use_w { "w3" } else { "x3" },
            "x4" => if use_w { "w4" } else { "x4" },
            "x5" => if use_w { "w5" } else { "x5" },
            "x6" => if use_w { "w6" } else { "x6" },
            "x7" => if use_w { "w7" } else { "x7" },
            _ => "x0",
        }
    }

    /// Parse a load instruction token into the actual ARM instruction and destination register.
    /// ARM's "ldr" instruction is width-polymorphic (the register determines access width),
    /// so load_instr_for_type returns "ldr32"/"ldr64" tokens to distinguish 32-bit from 64-bit.
    pub(super) fn arm_parse_load(instr: &'static str) -> (&'static str, &'static str) {
        match instr {
            "ldr32" => ("ldr", "w0"),
            "ldr64" => ("ldr", "x0"),
            "ldrb" | "ldrh" => (instr, "w0"),
            // ldrsb, ldrsh, ldrsw all sign-extend into x0
            _ => (instr, "x0"),
        }
    }

}
