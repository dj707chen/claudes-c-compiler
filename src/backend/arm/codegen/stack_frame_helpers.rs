//! Stack frame setup helpers for AArch64.
//!
//! Prologue/epilogue generation, callee-saved register scanning for
//! inline assembly, and register restore sequences.

use crate::ir::ir::{IrFunction, Instruction, Operand};
use crate::backend::regalloc::PhysReg;
use super::codegen::{ArmCodegen, callee_saved_name};
use super::asm_emitter::ARM_GP_SCRATCH;

impl ArmCodegen {
    /// Pre-scan all inline asm instructions in a function to predict which
    /// callee-saved registers will be needed as scratch registers.
    ///
    /// The inline asm scratch allocator (`assign_scratch_reg`) walks through
    /// `ARM_GP_SCRATCH` = [x9..x15, x19, x20, x21], skipping registers that
    /// appear in the clobber/excluded list. When enough caller-saved scratch regs
    /// (x9-x15) are clobbered, the allocator falls through to callee-saved
    /// registers (x19, x20, x21). These must be saved/restored in the prologue,
    /// but the prologue is emitted before inline asm codegen runs. This function
    /// simulates the allocation to discover the callee-saved registers early.
    pub(super) fn prescan_inline_asm_callee_saved(func: &IrFunction, used_callee_saved: &mut Vec<PhysReg>) {
        for block in &func.blocks {
            for instr in &block.instructions {
                if let Instruction::InlineAsm {
                    outputs, inputs, clobbers, ..
                } = instr {
                    // Build excluded set: clobber registers + specific constraint regs
                    let mut excluded: Vec<String> = Vec::new();
                    for clobber in clobbers {
                        if clobber == "cc" || clobber == "memory" {
                            continue;
                        }
                        excluded.push(clobber.clone());
                        // Also exclude the alternate width alias (wN <-> xN)
                        // and normalize rN (GCC AArch64 alias for xN)
                        if let Some(suffix) = clobber.strip_prefix('w') {
                            if suffix.chars().all(|c| c.is_ascii_digit()) {
                                excluded.push(format!("x{}", suffix));
                            }
                        } else if let Some(suffix) = clobber.strip_prefix('x') {
                            if suffix.chars().all(|c| c.is_ascii_digit()) {
                                excluded.push(format!("w{}", suffix));
                            }
                        } else if let Some(suffix) = clobber.strip_prefix('r') {
                            if suffix.chars().all(|c| c.is_ascii_digit()) {
                                if let Ok(n) = suffix.parse::<u32>() {
                                    if n <= 30 {
                                        excluded.push(format!("x{}", n));
                                        excluded.push(format!("w{}", n));
                                    }
                                }
                            }
                        }
                    }

                    // Count GP scratch registers needed:
                    // 1. GpReg operands (outputs + inputs that are "r" type, not tied, not specific)
                    // 2. Memory operands that need indirection (non-alloca pointers get a scratch reg)
                    let mut gp_scratch_needed = 0usize;

                    for (constraint, _, _) in outputs {
                        let c = constraint.trim_start_matches(['=', '+', '&']);
                        if c.starts_with('{') && c.ends_with('}') {
                            let reg_name = &c[1..c.len()-1];
                            // Normalize rN -> xN (GCC AArch64 alias)
                            let normalized = super::asm_emitter::normalize_aarch64_register(reg_name);
                            excluded.push(normalized);
                        } else if c == "m" || c == "Q" || c.contains('Q') || c.contains('m') {
                            // Memory operands may need a scratch reg for indirection.
                            // Conservatively count each one.
                            gp_scratch_needed += 1;
                        } else if c == "w" {
                            // FP register, doesn't consume GP scratch
                        } else if !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()) {
                            // Tied operand, doesn't need its own scratch
                        } else {
                            // GpReg
                            gp_scratch_needed += 1;
                        }
                    }

                    // Count "+" read-write outputs that generate synthetic inputs.
                    // Synthetic inputs from "+r" have constraint "r" and consume a
                    // GP scratch slot in phase 1 (even though the register is later
                    // overwritten by copy_metadata_from). We must count these too.
                    let num_plus = outputs.iter().filter(|(c,_,_)| c.contains('+')).count();
                    {
                        let mut plus_idx = 0;
                        for (constraint, _, _) in outputs.iter() {
                            if constraint.contains('+') {
                                let c = constraint.trim_start_matches(['=', '+', '&']);
                                // Synthetic input inherits constraint with '+' stripped
                                // "+r" → "r" (GpReg, consumes scratch), "+m" → "m" (Memory, skip)
                                if c != "m" && c != "Q" && !c.contains('Q') && !c.contains('m') && c != "w"
                                    && !(c.starts_with('{') && c.ends_with('}'))
                                    && (!c.chars().all(|ch| ch.is_ascii_digit()) || c.is_empty())
                                {
                                    gp_scratch_needed += 1;
                                }
                                plus_idx += 1;
                            }
                        }
                        let _ = plus_idx;
                    }

                    for (i, (constraint, val, _)) in inputs.iter().enumerate() {
                        // Skip synthetic inputs (they're already counted above)
                        if i < num_plus {
                            continue;
                        }
                        let c = constraint.trim_start_matches(['=', '+', '&']);
                        if c.starts_with('{') && c.ends_with('}') {
                            let reg_name = &c[1..c.len()-1];
                            // Normalize rN -> xN (GCC AArch64 alias)
                            let normalized = super::asm_emitter::normalize_aarch64_register(reg_name);
                            excluded.push(normalized);
                        } else if c == "m" || c == "Q" || c.contains('Q') || c.contains('m') {
                            gp_scratch_needed += 1;
                        } else if c == "w" {
                            // FP register
                        } else if !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()) {
                            // Tied operand
                        } else {
                            // Check if constant input with immediate-capable constraint
                            // would be promoted to Immediate (no scratch needed)
                            let is_const = matches!(val, Operand::Const(_));
                            let has_imm_alt = c.contains('i') || c.contains('I') || c.contains('n');
                            if is_const && has_imm_alt {
                                // Would be promoted to Immediate, no GP scratch needed
                            } else {
                                gp_scratch_needed += 1;
                            }
                        }
                    }

                    // Simulate walking through ARM_GP_SCRATCH, skipping excluded regs
                    let mut scratch_idx = 0;
                    let mut assigned = 0;
                    while assigned < gp_scratch_needed && scratch_idx < ARM_GP_SCRATCH.len() {
                        let reg = ARM_GP_SCRATCH[scratch_idx];
                        scratch_idx += 1;
                        if excluded.iter().any(|e| e == reg) {
                            continue;
                        }
                        assigned += 1;
                        // Check if this is a callee-saved register
                        if let Some(num_str) = reg.strip_prefix('x') {
                            if let Ok(n) = num_str.parse::<u8>() {
                                if (19..=28).contains(&n) {
                                    let phys = PhysReg(n);
                                    if !used_callee_saved.contains(&phys) {
                                        used_callee_saved.push(phys);
                                    }
                                }
                            }
                        }
                    }

                    // Also handle overflow beyond ARM_GP_SCRATCH (format!("x{}", 9 + idx))
                    while assigned < gp_scratch_needed {
                        let idx = scratch_idx;
                        scratch_idx += 1;
                        let reg_num = 9 + idx;
                        let reg_name = format!("x{}", reg_num);
                        if excluded.iter().any(|e| e == &reg_name) {
                            continue;
                        }
                        assigned += 1;
                        if (19..=28).contains(&reg_num) {
                            let phys = PhysReg(reg_num as u8);
                            if !used_callee_saved.contains(&phys) {
                                used_callee_saved.push(phys);
                            }
                        }
                    }
                }
            }
        }
        // Sort for deterministic prologue/epilogue emission
        used_callee_saved.sort_by_key(|r| r.0);
    }

    /// Restore callee-saved registers before epilogue.
    pub(super) fn emit_restore_callee_saved(&mut self) {
        let used_regs = self.used_callee_saved.clone();
        let base = self.callee_save_offset;
        let n = used_regs.len();
        let mut i = 0;
        while i + 1 < n {
            let r1 = callee_saved_name(used_regs[i]);
            let r2 = callee_saved_name(used_regs[i + 1]);
            let offset = base + (i as i64) * 8;
            self.emit_ldp_from_sp(r1, r2, offset);
            i += 2;
        }
        if i < n {
            let r = callee_saved_name(used_regs[i]);
            let offset = base + (i as i64) * 8;
            self.emit_load_from_sp(r, offset, "ldr");
        }
    }

    /// Emit function prologue: allocate stack and save fp/lr.
    pub(super) fn emit_prologue_arm(&mut self, frame_size: i64) {
        const PAGE_SIZE: i64 = 4096;
        if frame_size > 0 && frame_size <= 504 {
            self.state.emit_fmt(format_args!("    stp x29, x30, [sp, #-{}]!", frame_size));
        } else if frame_size > PAGE_SIZE {
            // Stack probing: for large frames, touch each page so the kernel
            // can grow the stack mapping. Without this, a single large sub
            // can skip guard pages and cause a segfault.
            let probe_label = self.state.fresh_label("stack_probe");
            self.emit_load_imm64("x17", frame_size);
            self.state.emit_fmt(format_args!("{}:", probe_label));
            self.state.emit_fmt(format_args!("    sub sp, sp, #{}", PAGE_SIZE));
            self.state.emit("    str xzr, [sp]");
            self.state.emit_fmt(format_args!("    sub x17, x17, #{}", PAGE_SIZE));
            self.state.emit_fmt(format_args!("    cmp x17, #{}", PAGE_SIZE));
            self.state.emit_fmt(format_args!("    b.hi {}", probe_label));
            self.state.emit("    sub sp, sp, x17");
            self.state.emit("    str xzr, [sp]");
            self.state.emit("    stp x29, x30, [sp]");
        } else {
            self.emit_sub_sp(frame_size);
            self.state.emit("    stp x29, x30, [sp]");
        }
        self.state.emit("    mov x29, sp");
    }

    /// Emit function epilogue: restore fp/lr and deallocate stack.
    pub(super) fn emit_epilogue_arm(&mut self, frame_size: i64) {
        if self.state.has_dyn_alloca {
            // DynAlloca modified SP at runtime; restore from frame pointer.
            self.state.emit("    mov sp, x29");
        }
        if frame_size > 0 && frame_size <= 504 {
            self.state.emit_fmt(format_args!("    ldp x29, x30, [sp], #{}", frame_size));
        } else {
            self.state.emit("    ldp x29, x30, [sp]");
            self.emit_add_sp(frame_size);
        }
    }

}
