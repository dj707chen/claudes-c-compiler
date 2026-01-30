//! Core operand loading and register manipulation helpers for i686.
//!
//! Handles loading IR operands into eax/ecx, storing results back to
//! stack slots, and mapping IR types to instruction mnemonics.

use crate::ir::ir::{IrConst, Operand, Value};
use crate::common::types::IrType;
use super::codegen::{I686Codegen, phys_reg_name};
use crate::emit;

impl I686Codegen {
    /// Load the address of va_list storage into %edx.
    ///
    /// va_list_ptr is an IR value that holds a pointer to the va_list storage.
    /// - If va_list_ptr is an alloca (local va_list variable), we LEA the slot
    ///   address into %edx (the alloca IS the va_list storage).
    /// - If va_list_ptr is a regular value (e.g., loaded pointer from va_list*),
    ///   we load its value into %edx (the value IS the address of va_list storage).
    pub(super) fn load_va_list_addr_to_edx(&mut self, va_list_ptr: &Value) {
        let is_alloca = self.state.is_alloca(va_list_ptr.0);
        if let Some(phys) = self.reg_assignments.get(&va_list_ptr.0).copied() {
            // Value is in a callee-saved register (non-alloca pointer value)
            let reg = phys_reg_name(phys);
            emit!(self.state, "    movl %{}, %edx", reg);
        } else if let Some(slot) = self.state.get_slot(va_list_ptr.0) {
            if is_alloca {
                // Alloca: the slot IS the va_list; get the address of the slot
                emit!(self.state, "    leal {}(%ebp), %edx", slot.0);
            } else {
                // Regular value: the slot holds a pointer to the va_list storage
                emit!(self.state, "    movl {}(%ebp), %edx", slot.0);
            }
        }
    }

    /// Load an operand into %eax.
    pub(super) fn operand_to_eax(&mut self, op: &Operand) {
        // Check register cache - skip load if value is already in eax
        if let Operand::Value(v) = op {
            let is_alloca = self.state.is_alloca(v.0);
            if self.state.reg_cache.acc_has(v.0, is_alloca) {
                return;
            }
        }

        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) => emit!(self.state, "    movl ${}, %eax", *v as i32),
                    IrConst::I16(v) => emit!(self.state, "    movl ${}, %eax", *v as i32),
                    IrConst::I32(v) => {
                        if *v == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else {
                            emit!(self.state, "    movl ${}, %eax", v);
                        }
                    }
                    IrConst::I64(v) => {
                        // On i686, we can only hold 32 bits in eax
                        // Truncate to low 32 bits
                        let low = *v as i32;
                        if low == 0 {
                            self.state.emit("    xorl %eax, %eax");
                        } else {
                            emit!(self.state, "    movl ${}, %eax", low);
                        }
                    }
                    IrConst::I128(v) => {
                        let low = *v as i32;
                        emit!(self.state, "    movl ${}, %eax", low);
                    }
                    IrConst::F32(fval) => emit!(self.state, "    movl ${}, %eax", fval.to_bits() as i32),
                    IrConst::F64(fval) => {
                        // Store low 32 bits of the f64 bit pattern
                        let low = fval.to_bits() as i32;
                        emit!(self.state, "    movl ${}, %eax", low);
                    }
                    IrConst::LongDouble(_, bytes) => {
                        // Load first 4 bytes of long double
                        let low = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                        emit!(self.state, "    movl ${}, %eax", low);
                    }
                    IrConst::Zero => {
                        self.state.emit("    xorl %eax, %eax");
                    }
                }
                self.state.reg_cache.invalidate_acc();
            }
            Operand::Value(v) => {
                let is_alloca = self.state.is_alloca(v.0);
                // Check if value is in a callee-saved register (allocas are never register-allocated)
                if let Some(phys) = self.reg_assignments.get(&v.0).copied() {
                    let reg = phys_reg_name(phys);
                    emit!(self.state, "    movl %{}, %eax", reg);
                    self.state.reg_cache.set_acc(v.0, false);
                } else if let Some(slot) = self.state.get_slot(v.0) {
                    if is_alloca {
                        // Alloca: the slot IS the data; load the address of the slot
                        if let Some(align) = self.state.alloca_over_align(v.0) {
                            // Over-aligned alloca: compute aligned address
                            emit!(self.state, "    leal {}(%ebp), %eax", slot.0);
                            emit!(self.state, "    addl ${}, %eax", align - 1);
                            emit!(self.state, "    andl ${}, %eax", -(align as i32));
                        } else {
                            emit!(self.state, "    leal {}(%ebp), %eax", slot.0);
                        }
                    } else {
                        // Regular value: load the value from the slot
                        emit!(self.state, "    movl {}(%ebp), %eax", slot.0);
                    }
                    self.state.reg_cache.set_acc(v.0, is_alloca);
                }
            }
        }
    }

    /// Load a 64-bit value's slot into %eax by OR'ing both 32-bit halves.
    /// Used for truthiness testing of I64/U64/F64 values on i686, where a value
    /// is nonzero iff either half is nonzero.
    pub(super) fn emit_wide_value_to_eax_ored(&mut self, value_id: u32) {
        if let Some(slot) = self.state.get_slot(value_id) {
            emit!(self.state, "    movl {}(%ebp), %eax", slot.0);
            emit!(self.state, "    orl {}(%ebp), %eax", slot.0 + 4);
        } else {
            // Wide values (I64/F64) on i686 should always have stack slots since
            // they can't fit in a single 32-bit register. Fall back to loading
            // the low 32 bits only as a last resort.
            self.operand_to_eax(&Operand::Value(Value(value_id)));
        }
    }

    /// Load an operand into %ecx.
    pub(super) fn operand_to_ecx(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) => emit!(self.state, "    movl ${}, %ecx", *v as i32),
                    IrConst::I16(v) => emit!(self.state, "    movl ${}, %ecx", *v as i32),
                    IrConst::I32(v) => {
                        if *v == 0 {
                            self.state.emit("    xorl %ecx, %ecx");
                        } else {
                            emit!(self.state, "    movl ${}, %ecx", v);
                        }
                    }
                    IrConst::I64(v) => {
                        let low = *v as i32;
                        if low == 0 {
                            self.state.emit("    xorl %ecx, %ecx");
                        } else {
                            emit!(self.state, "    movl ${}, %ecx", low);
                        }
                    }
                    IrConst::I128(v) => {
                        let low = *v as i32;
                        emit!(self.state, "    movl ${}, %ecx", low);
                    }
                    IrConst::F32(fval) => emit!(self.state, "    movl ${}, %ecx", fval.to_bits() as i32),
                    IrConst::F64(fval) => {
                        let low = fval.to_bits() as i32;
                        emit!(self.state, "    movl ${}, %ecx", low);
                    }
                    IrConst::LongDouble(_, bytes) => {
                        let low = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                        emit!(self.state, "    movl ${}, %ecx", low);
                    }
                    IrConst::Zero => {
                        self.state.emit("    xorl %ecx, %ecx");
                    }
                }
            }
            Operand::Value(v) => {
                let is_alloca = self.state.is_alloca(v.0);
                if let Some(phys) = self.reg_assignments.get(&v.0).copied() {
                    let reg = phys_reg_name(phys);
                    emit!(self.state, "    movl %{}, %ecx", reg);
                } else if let Some(slot) = self.state.get_slot(v.0) {
                    if is_alloca {
                        // Alloca: load the address of the slot
                        if let Some(align) = self.state.alloca_over_align(v.0) {
                            emit!(self.state, "    leal {}(%ebp), %ecx", slot.0);
                            emit!(self.state, "    addl ${}, %ecx", align - 1);
                            emit!(self.state, "    andl ${}, %ecx", -(align as i32));
                        } else {
                            emit!(self.state, "    leal {}(%ebp), %ecx", slot.0);
                        }
                    } else {
                        emit!(self.state, "    movl {}(%ebp), %ecx", slot.0);
                    }
                } else if self.state.reg_cache.acc_has(v.0, false) || self.state.reg_cache.acc_has(v.0, true) {
                    // Value is in accumulator (no stack slot) — move eax to ecx.
                    self.state.emit("    movl %eax, %ecx");
                } else {
                    self.state.emit("    xorl %ecx, %ecx");
                }
            }
        }
    }

    /// Store %eax to a value's destination (callee-saved register or stack slot).
    pub(super) fn store_eax_to(&mut self, dest: &Value) {
        if let Some(phys) = self.dest_reg(dest) {
            let reg = phys_reg_name(phys);
            emit!(self.state, "    movl %eax, %{}", reg);
            self.state.reg_cache.invalidate_acc();
        } else if let Some(slot) = self.state.get_slot(dest.0) {
            emit!(self.state, "    movl %eax, {}(%ebp)", slot.0);
            // If this dest is a wide value (I64/U64/F64), zero the upper 4 bytes.
            // Wide values occupy 8-byte slots, and other paths (e.g. Copy from
            // IrConst::I64) may write all 8 bytes. If we only write the low 4,
            // the upper half retains stack garbage, which corrupts truthiness
            // checks that OR both halves (emit_wide_value_to_eax_ored).
            if self.state.wide_values.contains(&dest.0) {
                emit!(self.state, "    movl $0, {}(%ebp)", slot.0 + 4);
            }
            self.state.reg_cache.set_acc(dest.0, false);
        }
    }

    /// Return the store mnemonic for a given type.
    pub(super) fn mov_store_for_type(&self, ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "movb",
            IrType::I16 | IrType::U16 => "movw",
            // On i686, pointer-sized types use movl (32-bit)
            _ => "movl",
        }
    }

    /// Return the load mnemonic for a given type.
    pub(super) fn mov_load_for_type(&self, ty: IrType) -> &'static str {
        match ty {
            IrType::I8 => "movsbl",    // sign-extend byte to 32-bit
            IrType::U8 => "movzbl",    // zero-extend byte to 32-bit
            IrType::I16 => "movswl",   // sign-extend word to 32-bit
            IrType::U16 => "movzwl",   // zero-extend word to 32-bit
            // Everything 32-bit or larger uses movl
            _ => "movl",
        }
    }

    /// Return the type suffix for an operation.
    pub(super) fn type_suffix(&self, ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "b",
            IrType::I16 | IrType::U16 => "w",
            // On i686, the default (pointer-sized) is "l" (32-bit)
            _ => "l",
        }
    }

    /// Return the register name for eax sub-register based on type size.
    pub(super) fn eax_for_type(&self, ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "%al",
            IrType::I16 | IrType::U16 => "%ax",
            _ => "%eax",
        }
    }

    /// Return the register name for ecx sub-register based on type size.
    pub(super) fn ecx_for_type(&self, ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "%cl",
            IrType::I16 | IrType::U16 => "%cx",
            _ => "%ecx",
        }
    }

    /// Return the register name for edx sub-register based on type size.
    pub(super) fn edx_for_type(&self, ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "%dl",
            IrType::I16 | IrType::U16 => "%dx",
            _ => "%edx",
        }
    }

    /// Check if an operand is a constant that fits in an i32 immediate.
    pub(super) fn const_as_imm32(op: &Operand) -> Option<i64> {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) => Some(*v as i64),
                    IrConst::I16(v) => Some(*v as i64),
                    IrConst::I32(v) => Some(*v as i64),
                    IrConst::I64(v) => {
                        // On i686, check if the value fits in 32 bits
                        if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
                            Some(*v)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Extract an immediate integer value from an operand.
    /// Used for SSE/AES instructions that require compile-time immediate operands.
    pub(super) fn operand_to_imm_i64(op: &Operand) -> i64 {
        match op {
            Operand::Const(c) => match c {
                IrConst::I8(v) => *v as i64,
                IrConst::I16(v) => *v as i64,
                IrConst::I32(v) => *v as i64,
                IrConst::I64(v) => *v,
                _ => 0,
            },
            Operand::Value(_) => 0,
        }
    }

}
