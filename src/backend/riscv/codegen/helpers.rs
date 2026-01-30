//! Core operand loading, memory access, and stack frame helpers for RISC-V 64.
//!
//! Handles s0-relative load/store with large offset support (via t6 scratch),
//! operand loading into t0 (accumulator), prologue/epilogue generation,
//! and type-to-instruction mapping.

use crate::ir::ir::{IrConst, Operand, Value};
use crate::common::types::IrType;
use super::codegen::{RiscvCodegen, callee_saved_name};

impl RiscvCodegen {
    /// Emit `.option norelax` if -mno-relax is set. Must be called once
    /// before any code is generated. This prevents the GNU assembler from
    /// generating R_RISCV_RELAX relocation entries, which is required for
    /// EFI stub code that forbids absolute symbol references from linker
    /// relaxation.
    pub(crate) fn emit_pre_directives(&mut self) {
        if self.no_relax {
            self.state.emit(".option norelax");
        }
    }

    /// Check if an immediate fits in a 12-bit signed field.
    pub(super) fn fits_imm12(val: i64) -> bool {
        (-2048..=2047).contains(&val)
    }

    /// Emit: store `reg` to `offset(s0)`, handling large offsets via t6.
    /// Uses t6 as scratch to avoid conflicts with t3-t5 call argument temps.
    pub(super) fn emit_store_to_s0(&mut self, reg: &str, offset: i64, store_instr: &str) {
        if Self::fits_imm12(offset) {
            self.state.emit_fmt(format_args!("    {} {}, {}(s0)", store_instr, reg, offset));
        } else {
            self.state.emit_fmt(format_args!("    li t6, {}", offset));
            self.state.emit("    add t6, s0, t6");
            self.state.emit_fmt(format_args!("    {} {}, 0(t6)", store_instr, reg));
        }
    }

    /// Emit: load from `offset(base)` into `dest`, handling large offsets via t6.
    /// For arbitrary base registers (not s0/sp). Uses t6 as scratch when offset
    /// exceeds RISC-V's 12-bit signed immediate range (-2048..2047).
    pub(super) fn emit_load_from_reg(state: &mut crate::backend::state::CodegenState, dest: &str, base: &str, offset: i64, load_instr: &str) {
        if Self::fits_imm12(offset) {
            state.emit_fmt(format_args!("    {} {}, {}({})", load_instr, dest, offset, base));
        } else {
            state.emit_fmt(format_args!("    li t6, {}", offset));
            state.emit_fmt(format_args!("    add t6, {}, t6", base));
            state.emit_fmt(format_args!("    {} {}, 0(t6)", load_instr, dest));
        }
    }

    /// Emit: load from `offset(s0)` into `reg`, handling large offsets via t6.
    /// Uses t6 as scratch to avoid conflicts with t3-t5 call argument temps.
    pub(super) fn emit_load_from_s0(&mut self, reg: &str, offset: i64, load_instr: &str) {
        if Self::fits_imm12(offset) {
            self.state.emit_fmt(format_args!("    {} {}, {}(s0)", load_instr, reg, offset));
        } else {
            self.state.emit_fmt(format_args!("    li t6, {}", offset));
            self.state.emit("    add t6, s0, t6");
            self.state.emit_fmt(format_args!("    {} {}, 0(t6)", load_instr, reg));
        }
    }

    /// Emit: `dest_reg = s0 + offset`, handling large offsets.
    pub(super) fn emit_addi_s0(&mut self, dest_reg: &str, offset: i64) {
        if Self::fits_imm12(offset) {
            self.state.emit_fmt(format_args!("    addi {}, s0, {}", dest_reg, offset));
        } else {
            self.state.emit_fmt(format_args!("    li {}, {}", dest_reg, offset));
            self.state.emit_fmt(format_args!("    add {}, s0, {}", dest_reg, dest_reg));
        }
    }

    /// Emit: store `reg` to `offset(sp)`, handling large offsets via t6.
    /// Used for stack overflow arguments in emit_call.
    pub(super) fn emit_store_to_sp(&mut self, reg: &str, offset: i64, store_instr: &str) {
        if Self::fits_imm12(offset) {
            self.state.emit_fmt(format_args!("    {} {}, {}(sp)", store_instr, reg, offset));
        } else {
            self.state.emit_fmt(format_args!("    li t6, {}", offset));
            self.state.emit("    add t6, sp, t6");
            self.state.emit_fmt(format_args!("    {} {}, 0(t6)", store_instr, reg));
        }
    }

    /// Emit: load from `offset(sp)` into `reg`, handling large offsets via t6.
    /// Used for loading stack overflow arguments in emit_call.
    pub(super) fn emit_load_from_sp(&mut self, reg: &str, offset: i64, load_instr: &str) {
        if Self::fits_imm12(offset) {
            self.state.emit_fmt(format_args!("    {} {}, {}(sp)", load_instr, reg, offset));
        } else {
            self.state.emit_fmt(format_args!("    li t6, {}", offset));
            self.state.emit("    add t6, sp, t6");
            self.state.emit_fmt(format_args!("    {} {}, 0(t6)", load_instr, reg));
        }
    }

    /// Emit: `sp = sp + imm`, handling large immediates via t6.
    /// Positive imm deallocates stack, negative allocates.
    pub(super) fn emit_addi_sp(&mut self, imm: i64) {
        if Self::fits_imm12(imm) {
            self.state.emit_fmt(format_args!("    addi sp, sp, {}", imm));
        } else if imm > 0 {
            self.state.emit_fmt(format_args!("    li t6, {}", imm));
            self.state.emit("    add sp, sp, t6");
        } else {
            self.state.emit_fmt(format_args!("    li t6, {}", -imm));
            self.state.emit("    sub sp, sp, t6");
        }
    }

    /// Emit prologue: allocate stack and save ra/s0.
    ///
    /// Stack layout (s0 points to top of frame = old sp):
    ///   s0 - 8:  saved ra
    ///   s0 - 16: saved s0
    ///   s0 - 16 - ...: local data (allocas and value slots)
    ///   sp: bottom of frame
    pub(super) fn emit_prologue_riscv(&mut self, frame_size: i64) {
        // For variadic functions, the register save area (64 bytes for a0-a7) is
        // placed ABOVE s0, contiguous with the caller's stack-passed arguments.
        // Layout: s0+0..s0+56 = a0..a7, s0+64+ = caller stack args.
        // This means total_alloc = frame_size + 64 for variadic, but s0 = sp + frame_size.
        let total_alloc = if self.is_variadic { frame_size + 64 } else { frame_size };

        const PAGE_SIZE: i64 = 4096;

        // Small-frame path requires ALL immediates to fit in 12 bits:
        // -total_alloc (sp adjust), and frame_size (s0 setup).
        if Self::fits_imm12(-total_alloc) && Self::fits_imm12(total_alloc) {
            // Small frame: all offsets fit in 12-bit immediates
            self.state.emit_fmt(format_args!("    addi sp, sp, -{}", total_alloc));
            // ra and s0 are saved relative to s0, which is sp + frame_size
            // (NOT sp + total_alloc for variadic functions!)
            self.state.emit_fmt(format_args!("    sd ra, {}(sp)", frame_size - 8));
            self.state.emit_fmt(format_args!("    sd s0, {}(sp)", frame_size - 16));
            self.state.emit_fmt(format_args!("    addi s0, sp, {}", frame_size));
        } else if total_alloc > PAGE_SIZE {
            // Stack probing: for large frames, touch each page so the kernel
            // can grow the stack mapping. Without this, a single large sub
            // can skip guard pages and cause a segfault.
            let probe_label = self.state.fresh_label("stack_probe");
            self.state.emit_fmt(format_args!("    li t1, {}", total_alloc));
            self.state.emit_fmt(format_args!("    li t2, {}", PAGE_SIZE));
            self.state.emit_fmt(format_args!("{}:", probe_label));
            self.state.emit("    sub sp, sp, t2");
            self.state.emit("    sd zero, 0(sp)");
            self.state.emit("    sub t1, t1, t2");
            self.state.emit_fmt(format_args!("    bgt t1, t2, {}", probe_label));
            self.state.emit("    sub sp, sp, t1");
            self.state.emit("    sd zero, 0(sp)");
            // Compute s0 = sp + frame_size (NOT total_alloc)
            self.state.emit_fmt(format_args!("    li t0, {}", frame_size));
            self.state.emit("    add t0, sp, t0");
            // Save ra and old s0 at s0-8, s0-16
            self.state.emit("    sd ra, -8(t0)");
            self.state.emit("    sd s0, -16(t0)");
            self.state.emit("    mv s0, t0");
        } else {
            // Large frame: use t0 for offsets
            self.state.emit_fmt(format_args!("    li t0, {}", total_alloc));
            self.state.emit("    sub sp, sp, t0");
            // Compute s0 = sp + frame_size (NOT total_alloc)
            self.state.emit_fmt(format_args!("    li t0, {}", frame_size));
            self.state.emit("    add t0, sp, t0");
            // Save ra and old s0 at s0-8, s0-16
            self.state.emit("    sd ra, -8(t0)");
            self.state.emit("    sd s0, -16(t0)");
            self.state.emit("    mv s0, t0");
        }
    }

    /// Emit epilogue: restore ra/s0 and deallocate stack.
    pub(super) fn emit_epilogue_riscv(&mut self, frame_size: i64) {
        let total_alloc = if self.is_variadic { frame_size + 64 } else { frame_size };
        // When DynAlloca is used, SP was modified at runtime, so we must restore
        // from s0 (frame pointer) rather than using SP-relative offsets.
        if !self.state.has_dyn_alloca && Self::fits_imm12(-total_alloc) && Self::fits_imm12(total_alloc) {
            // Small frame: restore from known sp offsets
            // ra/s0 saved at sp + frame_size - 8/16 (relative to current sp)
            self.state.emit_fmt(format_args!("    ld ra, {}(sp)", frame_size - 8));
            self.state.emit_fmt(format_args!("    ld s0, {}(sp)", frame_size - 16));
            self.state.emit_fmt(format_args!("    addi sp, sp, {}", total_alloc));
        } else {
            // Large frame or DynAlloca: restore from s0-relative offsets (always fit in imm12).
            self.state.emit("    ld ra, -8(s0)");
            self.state.emit("    ld t0, -16(s0)");
            // For variadic functions, s0 + 64 = old_sp, so sp = s0 + 64
            if self.is_variadic {
                self.state.emit("    addi sp, s0, 64");
            } else {
                self.state.emit("    mv sp, s0");
            }
            self.state.emit("    mv s0, t0");
        }
    }

    /// Load an operand into t0.
    pub(super) fn operand_to_t0(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                self.state.reg_cache.invalidate_acc();
                match c {
                    IrConst::I8(v) => self.state.emit_fmt(format_args!("    li t0, {}", v)),
                    IrConst::I16(v) => self.state.emit_fmt(format_args!("    li t0, {}", v)),
                    IrConst::I32(v) => self.state.emit_fmt(format_args!("    li t0, {}", v)),
                    IrConst::I64(v) => self.state.emit_fmt(format_args!("    li t0, {}", v)),
                    IrConst::F32(v) => {
                        let bits = v.to_bits() as u64;
                        self.state.emit_fmt(format_args!("    li t0, {}", bits as i64));
                    }
                    IrConst::F64(v) => {
                        let bits = v.to_bits();
                        self.state.emit_fmt(format_args!("    li t0, {}", bits as i64));
                    }
                    // LongDouble at computation level is treated as F64
                    IrConst::LongDouble(v, _) => {
                        let bits = v.to_bits();
                        self.state.emit_fmt(format_args!("    li t0, {}", bits as i64));
                    }
                    IrConst::I128(v) => self.state.emit_fmt(format_args!("    li t0, {}", *v as i64)), // truncate
                    IrConst::Zero => self.state.emit("    li t0, 0"),
                }
            }
            Operand::Value(v) => {
                let is_alloca = self.state.is_alloca(v.0);
                if self.state.reg_cache.acc_has(v.0, is_alloca) {
                    return; // Cache hit — t0 already holds this value.
                }
                // Check if this value is register-allocated.
                if let Some(&reg) = self.reg_assignments.get(&v.0) {
                    let reg_name = callee_saved_name(reg);
                    self.state.emit_fmt(format_args!("    mv t0, {}", reg_name));
                    self.state.reg_cache.set_acc(v.0, false);
                } else if let Some(slot) = self.state.get_slot(v.0) {
                    if is_alloca {
                        if let Some(align) = self.state.alloca_over_align(v.0) {
                            // Over-aligned alloca: compute aligned address.
                            // t0 = (slot_addr + align-1) & -align
                            self.emit_addi_s0("t0", slot.0);
                            self.state.emit_fmt(format_args!("    li t6, {}", align - 1));
                            self.state.emit("    add t0, t0, t6");
                            self.state.emit_fmt(format_args!("    li t6, -{}", align));
                            self.state.emit("    and t0, t0, t6");
                        } else {
                            self.emit_addi_s0("t0", slot.0);
                        }
                    } else {
                        self.emit_load_from_s0("t0", slot.0, "ld");
                    }
                    self.state.reg_cache.set_acc(v.0, is_alloca);
                } else if self.state.reg_cache.acc_has(v.0, false) || self.state.reg_cache.acc_has(v.0, true) {
                    // Value has no slot or register but is in the accumulator cache
                    // (skip-slot optimization: immediately-consumed values stay in t0).
                    return;
                } else {
                    self.state.emit("    li t0, 0");
                    self.state.reg_cache.invalidate_acc();
                }
            }
        }
    }

    /// Store t0 to a value's location (register or stack slot).
    /// Register-only strategy: if the value has a callee-saved register assignment,
    /// store ONLY to the register (skip the stack write). This eliminates redundant
    /// memory stores for register-allocated values.
    pub(super) fn store_t0_to(&mut self, dest: &Value) {
        if let Some(&reg) = self.reg_assignments.get(&dest.0) {
            // Value has a callee-saved register: store only to register, skip stack.
            let reg_name = callee_saved_name(reg);
            self.state.emit_fmt(format_args!("    mv {}, t0", reg_name));
        } else if let Some(slot) = self.state.get_slot(dest.0) {
            // No register: store to stack slot.
            self.emit_store_to_s0("t0", slot.0, "sd");
        }
        self.state.reg_cache.set_acc(dest.0, false);
    }

    pub(super) fn store_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "sb",
            IrType::I16 | IrType::U16 => "sh",
            IrType::I32 | IrType::U32 | IrType::F32 => "sw",
            _ => "sd",
        }
    }

    pub(super) fn load_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 => "lb",
            IrType::U8 => "lbu",
            IrType::I16 => "lh",
            IrType::U16 => "lhu",
            IrType::I32 => "lw",
            IrType::U32 | IrType::F32 => "lwu",
            _ => "ld",
        }
    }

    /// Load the address of a pointer Value into the given register.
    pub(super) fn load_ptr_to_reg_rv(&mut self, ptr: &Value, reg: &str) {
        if let Some(slot) = self.state.get_slot(ptr.0) {
            if self.state.is_alloca(ptr.0) {
                self.emit_addi_s0(reg, slot.0);
            } else {
                self.emit_load_from_s0(reg, slot.0, "ld");
            }
        }
    }

}
