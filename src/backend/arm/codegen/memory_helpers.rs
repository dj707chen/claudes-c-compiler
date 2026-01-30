//! Memory access helpers for AArch64.
//!
//! SP/FP-relative load/store with large offset handling (via x17 scratch),
//! STP/LDP pair operations, and large immediate loading (movz/movk sequence).

use crate::ir::ir::Value;
use super::codegen::ArmCodegen;

impl ArmCodegen {
    /// Emit a large immediate subtraction from sp. Uses x17 (IP1) as scratch.
    pub(super) fn emit_sub_sp(&mut self, n: i64) {
        if n == 0 { return; }
        if n <= 4095 {
            self.state.emit_fmt(format_args!("    sub sp, sp, #{}", n));
        } else {
            self.emit_load_imm64("x17", n);
            self.state.emit("    sub sp, sp, x17");
        }
    }

    /// Emit a large immediate addition to sp. Uses x17 (IP1) as scratch.
    pub(super) fn emit_add_sp(&mut self, n: i64) {
        if n == 0 { return; }
        if n <= 4095 {
            self.state.emit_fmt(format_args!("    add sp, sp, #{}", n));
        } else {
            self.emit_load_imm64("x17", n);
            self.state.emit("    add sp, sp, x17");
        }
    }

    /// Get the access size in bytes for an AArch64 load/store instruction and register.
    /// For str/ldr, the access size depends on the register:
    /// w registers = 4 bytes, x registers = 8 bytes,
    /// s (single-precision float) = 4 bytes, d (double-precision float) = 8 bytes,
    /// q (SIMD/quad) = 16 bytes.
    pub(super) fn access_size_for_instr(instr: &str, reg: &str) -> i64 {
        match instr {
            "strb" | "ldrb" | "ldrsb" => 1,
            "strh" | "ldrh" | "ldrsh" => 2,
            "ldrsw" => 4,
            "str" | "ldr" => {
                if reg.starts_with('w') || reg.starts_with('s') {
                    4
                } else if reg.starts_with('q') {
                    16
                } else {
                    // x registers and d registers are both 8 bytes
                    8
                }
            }
            _ => 1, // conservative default
        }
    }

    /// Check if an offset is valid for unsigned immediate addressing on AArch64.
    /// The unsigned offset is a 12-bit field scaled by access size: max = 4095 * access_size.
    /// The offset must also be naturally aligned to the access size.
    pub(super) fn is_valid_imm_offset(offset: i64, instr: &str, reg: &str) -> bool {
        if offset < 0 { return false; }
        let access_size = Self::access_size_for_instr(instr, reg);
        let max_offset = 4095 * access_size;
        offset <= max_offset && offset % access_size == 0
    }

    /// Emit store to [base, #offset], handling large offsets.
    /// For large frames with x19 as frame base register, tries x19-relative addressing
    /// before falling back to the expensive movz+movk+add sequence.
    pub(super) fn emit_store_to_sp(&mut self, reg: &str, offset: i64, instr: &str) {
        // When DynAlloca is present, use x29 (frame pointer) as base.
        let base = if self.state.has_dyn_alloca { "x29" } else { "sp" };
        if Self::is_valid_imm_offset(offset, instr, reg) {
            self.state.emit_fmt(format_args!("    {} {}, [{}, #{}]", instr, reg, base, offset));
        } else if let Some(fb_offset) = self.frame_base_offset {
            // Try x19-relative addressing (x19 = sp + frame_base_offset)
            let rel_offset = offset - fb_offset;
            if Self::is_valid_imm_offset(rel_offset, instr, reg) {
                self.state.emit_fmt(format_args!("    {} {}, [x19, #{}]", instr, reg, rel_offset));
            } else {
                self.load_large_imm("x17", offset);
                self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
                self.state.emit_fmt(format_args!("    {} {}, [x17]", instr, reg));
            }
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
            self.state.emit_fmt(format_args!("    {} {}, [x17]", instr, reg));
        }
    }

    /// Emit load from [base, #offset], handling large offsets.
    /// For large frames with x19 as frame base register, tries x19-relative addressing.
    pub(super) fn emit_load_from_sp(&mut self, reg: &str, offset: i64, instr: &str) {
        let base = if self.state.has_dyn_alloca { "x29" } else { "sp" };
        if Self::is_valid_imm_offset(offset, instr, reg) {
            self.state.emit_fmt(format_args!("    {} {}, [{}, #{}]", instr, reg, base, offset));
        } else if let Some(fb_offset) = self.frame_base_offset {
            let rel_offset = offset - fb_offset;
            if Self::is_valid_imm_offset(rel_offset, instr, reg) {
                self.state.emit_fmt(format_args!("    {} {}, [x19, #{}]", instr, reg, rel_offset));
            } else {
                self.load_large_imm("x17", offset);
                self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
                self.state.emit_fmt(format_args!("    {} {}, [x17]", instr, reg));
            }
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
            self.state.emit_fmt(format_args!("    {} {}, [x17]", instr, reg));
        }
    }

    /// Emit store to [sp, #offset] using the REAL sp register, even when alloca is present.
    /// Used for storing into dynamically-allocated call stack arg areas that live at the
    /// current sp, NOT in the frame (x29-relative).
    pub(super) fn emit_store_to_raw_sp(&mut self, reg: &str, offset: i64, instr: &str) {
        if Self::is_valid_imm_offset(offset, instr, reg) {
            self.state.emit_fmt(format_args!("    {} {}, [sp, #{}]", instr, reg, offset));
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit("    add x17, sp, x17");
            self.state.emit_fmt(format_args!("    {} {}, [x17]", instr, reg));
        }
    }

    /// Emit `stp reg1, reg2, [base, #offset]` handling large offsets.
    /// Uses x19 frame base for large frames when possible.
    pub(super) fn emit_stp_to_sp(&mut self, reg1: &str, reg2: &str, offset: i64) {
        let base = if self.state.has_dyn_alloca { "x29" } else { "sp" };
        // stp supports signed offsets in [-512, 504] range (multiples of 8)
        if (-512..=504).contains(&offset) {
            self.state.emit_fmt(format_args!("    stp {}, {}, [{}, #{}]", reg1, reg2, base, offset));
        } else if let Some(fb_offset) = self.frame_base_offset {
            let rel = offset - fb_offset;
            if (-512..=504).contains(&rel) {
                self.state.emit_fmt(format_args!("    stp {}, {}, [x19, #{}]", reg1, reg2, rel));
            } else {
                self.load_large_imm("x17", offset);
                self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
                self.state.emit_fmt(format_args!("    stp {}, {}, [x17]", reg1, reg2));
            }
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
            self.state.emit_fmt(format_args!("    stp {}, {}, [x17]", reg1, reg2));
        }
    }

    pub(super) fn emit_ldp_from_sp(&mut self, reg1: &str, reg2: &str, offset: i64) {
        let base = if self.state.has_dyn_alloca { "x29" } else { "sp" };
        if (-512..=504).contains(&offset) {
            self.state.emit_fmt(format_args!("    ldp {}, {}, [{}, #{}]", reg1, reg2, base, offset));
        } else if let Some(fb_offset) = self.frame_base_offset {
            let rel = offset - fb_offset;
            if (-512..=504).contains(&rel) {
                self.state.emit_fmt(format_args!("    ldp {}, {}, [x19, #{}]", reg1, reg2, rel));
            } else {
                self.load_large_imm("x17", offset);
                self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
                self.state.emit_fmt(format_args!("    ldp {}, {}, [x17]", reg1, reg2));
            }
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
            self.state.emit_fmt(format_args!("    ldp {}, {}, [x17]", reg1, reg2));
        }
    }

    /// Emit `add dest, sp, #offset` handling large offsets.
    /// Uses x19 frame base when available, falls back to x17 scratch.
    pub(super) fn emit_add_sp_offset(&mut self, dest: &str, offset: i64) {
        let base = if self.state.has_dyn_alloca { "x29" } else { "sp" };
        if (0..=4095).contains(&offset) {
            self.state.emit_fmt(format_args!("    add {}, {}, #{}", dest, base, offset));
        } else if let Some(fb_offset) = self.frame_base_offset {
            let rel = offset - fb_offset;
            if (0..=4095).contains(&rel) {
                self.state.emit_fmt(format_args!("    add {}, x19, #{}", dest, rel));
            } else if (-4095..0).contains(&rel) {
                self.state.emit_fmt(format_args!("    sub {}, x19, #{}", dest, -rel));
            } else {
                self.load_large_imm("x17", offset);
                self.state.emit_fmt(format_args!("    add {}, {}, x17", dest, base));
            }
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add {}, {}, x17", dest, base));
        }
    }

    /// Emit `add dest, x29, #offset` handling large offsets.
    /// Uses x17 (IP1) as scratch for offsets > 4095.
    pub(super) fn emit_add_fp_offset(&mut self, dest: &str, offset: i64) {
        if (0..=4095).contains(&offset) {
            self.state.emit_fmt(format_args!("    add {}, x29, #{}", dest, offset));
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add {}, x29, x17", dest));
        }
    }

    /// Emit load from an arbitrary base register with offset, handling large offsets via x17.
    /// For offsets that exceed the ARM64 unsigned immediate range, materializes the
    /// effective address into x17 and loads from [x17].
    pub(super) fn emit_load_from_reg(&mut self, dest: &str, base: &str, offset: i64, instr: &str) {
        if Self::is_valid_imm_offset(offset, instr, dest) {
            self.state.emit_fmt(format_args!("    {} {}, [{}, #{}]", instr, dest, base, offset));
        } else {
            self.load_large_imm("x17", offset);
            self.state.emit_fmt(format_args!("    add x17, {}, x17", base));
            self.state.emit_fmt(format_args!("    {} {}, [x17]", instr, dest));
        }
    }

    /// Load an immediate into a register using the most efficient sequence.
    /// Handles all 64-bit values including negatives via MOVZ/MOVK or MOVN/MOVK.
    pub(super) fn load_large_imm(&mut self, reg: &str, val: i64) {
        self.emit_load_imm64(reg, val);
    }

    /// Load a 64-bit immediate value into a register using movz/movn + movk sequence.
    /// Uses MOVN (move-not) for values where most halfwords are 0xFFFF, which
    /// gives shorter sequences for negative numbers and large values.
    pub(super) fn emit_load_imm64(&mut self, reg: &str, val: i64) {
        let bits = val as u64;
        if bits == 0 {
            self.state.emit_fmt(format_args!("    mov {}, #0", reg));
            return;
        }
        if bits == 0xFFFFFFFF_FFFFFFFF {
            // All-ones: MOVN reg, #0 produces NOT(0) = 0xFFFFFFFFFFFFFFFF
            self.state.emit_fmt(format_args!("    movn {}, #0", reg));
            return;
        }

        // Extract 16-bit halfwords
        let hw: [u16; 4] = [
            (bits & 0xffff) as u16,
            ((bits >> 16) & 0xffff) as u16,
            ((bits >> 32) & 0xffff) as u16,
            ((bits >> 48) & 0xffff) as u16,
        ];

        // Count how many halfwords are 0x0000 vs 0xFFFF to pick MOVZ vs MOVN
        let zeros = hw.iter().filter(|&&h| h == 0x0000).count();
        let ones = hw.iter().filter(|&&h| h == 0xFFFF).count();

        if ones > zeros {
            // Use MOVN (move-not) strategy: start with all-ones, patch non-0xFFFF halfwords
            // MOVN sets the register to NOT(imm16 << shift)
            let mut first = true;
            for (i, &h) in hw.iter().enumerate() {
                if h != 0xFFFF {
                    let shift = i * 16;
                    let not_h = (!h) as u64 & 0xffff;
                    if first {
                        if shift == 0 {
                            self.state.emit_fmt(format_args!("    movn {}, #{}", reg, not_h));
                        } else {
                            self.state.emit_fmt(format_args!("    movn {}, #{}, lsl #{}", reg, not_h, shift));
                        }
                        first = false;
                    } else if shift == 0 {
                        self.state.emit_fmt(format_args!("    movk {}, #{}", reg, h as u64));
                    } else {
                        self.state.emit_fmt(format_args!("    movk {}, #{}, lsl #{}", reg, h as u64, shift));
                    }
                }
            }
        } else {
            // Use MOVZ (move-zero) strategy: start with all-zeros, patch non-0x0000 halfwords
            let mut first = true;
            for (i, &h) in hw.iter().enumerate() {
                if h != 0x0000 {
                    let shift = i * 16;
                    if first {
                        if shift == 0 {
                            self.state.emit_fmt(format_args!("    movz {}, #{}", reg, h as u64));
                        } else {
                            self.state.emit_fmt(format_args!("    movz {}, #{}, lsl #{}", reg, h as u64, shift));
                        }
                        first = false;
                    } else if shift == 0 {
                        self.state.emit_fmt(format_args!("    movk {}, #{}", reg, h as u64));
                    } else {
                        self.state.emit_fmt(format_args!("    movk {}, #{}, lsl #{}", reg, h as u64, shift));
                    }
                }
            }
        }
    }

    /// Load the address represented by a pointer Value into the given register.
    /// For alloca values, computes the address; for others, loads the stored pointer.
    pub(super) fn load_ptr_to_reg(&mut self, ptr: &Value, reg: &str) {
        if let Some(slot) = self.state.get_slot(ptr.0) {
            if self.state.is_alloca(ptr.0) {
                self.emit_add_sp_offset(reg, slot.0);
            } else {
                self.emit_load_from_sp(reg, slot.0, "ldr");
            }
        }
    }

    /// Add an immediate offset to x17. Used by F128 load/store paths that
    /// use x17 as the address register instead of x9 (which emit_add_offset_to_addr_reg uses).
    pub(super) fn emit_add_imm_to_x17(&mut self, offset: i64) {
        if (0..=4095).contains(&offset) {
            self.state.emit_fmt(format_args!("    add x17, x17, #{}", offset));
        } else if offset < 0 && (-offset) <= 4095 {
            self.state.emit_fmt(format_args!("    sub x17, x17, #{}", -offset));
        } else {
            // Use x9 as a temp to load the large immediate, then add to x17
            self.load_large_imm("x9", offset);
            self.state.emit("    add x17, x17, x9");
        }
    }

}
