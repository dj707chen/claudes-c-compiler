//! Function call helpers for i686 (cdecl and fastcall conventions).
//!
//! Handles stack argument emission, fastcall register classification,
//! and callee-saved register clobber annotations.

use crate::ir::ir::{IrConst, IrFunction, Operand, Value};
use crate::common::types::IrType;
use crate::backend::generation::is_i128_type;
use crate::backend::traits::ArchCodegen;
use super::codegen::I686Codegen;
use crate::emit;

impl I686Codegen {
    /// Check if a param type is eligible for fastcall register passing.
    /// Only DWORD-sized or smaller integer/pointer types qualify.
    pub(super) fn is_fastcall_reg_eligible(&self, ty: IrType) -> bool {
        matches!(ty, IrType::I8 | IrType::U8 | IrType::I16 | IrType::U16 |
                     IrType::I32 | IrType::U32 | IrType::Ptr)
    }

    /// Count how many leading params are passed in registers for fastcall (max 2).
    pub(super) fn count_fastcall_reg_params(&self, func: &IrFunction) -> usize {
        let mut count = 0;
        for param in &func.params {
            if count >= 2 { break; }
            let ty = param.ty;
            if self.is_fastcall_reg_eligible(ty) {
                count += 1;
            } else {
                break; // non-eligible param stops register assignment
            }
        }
        count
    }

    /// Emit comments for callee-saved registers clobbered by inline asm.
    pub(super) fn emit_callee_saved_clobber_annotations(&mut self, clobbers: &[String]) {
        for clobber in clobbers {
            let reg_name = match clobber.as_str() {
                "ebx" | "bx" | "bl" | "bh" => Some("%ebx"),
                "esi" | "si" => Some("%esi"),
                "edi" | "di" => Some("%edi"),
                _ => None,
            };
            if let Some(reg) = reg_name {
                self.state.emit_fmt(format_args!("    # asm clobber {}", reg));
            }
        }
    }

    /// Emit a fastcall function call on i686.
    /// First two DWORD (int/ptr) args go in ECX, EDX.
    /// Remaining args go on the stack (right-to-left push order).
    /// The callee pops stack args, so caller does NOT adjust ESP after call.
    pub(super) fn emit_fastcall(&mut self, args: &[Operand], arg_types: &[IrType],
                     direct_name: Option<&str>, func_ptr: Option<&Operand>,
                     dest: Option<Value>, return_type: IrType) {
        let indirect = func_ptr.is_some() && direct_name.is_none();

        // Determine which args go in registers vs stack.
        let mut reg_count = 0usize;
        for ty in arg_types.iter() {
            if reg_count >= 2 { break; }
            if self.is_fastcall_reg_eligible(*ty) {
                reg_count += 1;
            } else {
                break;
            }
        }

        // Compute stack space for overflow args (args beyond the register ones).
        let mut stack_bytes = 0usize;
        for i in reg_count..args.len() {
            let ty = if i < arg_types.len() { arg_types[i] } else { IrType::I32 };
            match ty {
                IrType::F64 | IrType::I64 | IrType::U64 => stack_bytes += 8,
                IrType::F128 => stack_bytes += 12,
                _ if is_i128_type(ty) => stack_bytes += 16,
                _ => stack_bytes += 4,
            }
        }
        // Align to 16 bytes
        let stack_arg_space = (stack_bytes + 15) & !15;

        // Spill indirect function pointer before stack manipulation.
        if indirect {
            self.emit_call_spill_fptr(func_ptr.expect("indirect call requires func_ptr"));
        }

        // Phase 1: Allocate stack space and write stack args.
        if stack_arg_space > 0 {
            emit!(self.state, "    subl ${}, %esp", stack_arg_space);
        }

        // Write stack args (skipping register args).
        let mut offset = 0i64;
        for i in reg_count..args.len() {
            let ty = if i < arg_types.len() { arg_types[i] } else { IrType::I32 };
            let arg = &args[i];

            match ty {
                IrType::I64 | IrType::U64 | IrType::F64 => {
                    self.emit_load_acc_pair(arg);
                    emit!(self.state, "    movl %eax, {}(%esp)", offset);
                    emit!(self.state, "    movl %edx, {}(%esp)", offset + 4);
                    offset += 8;
                }
                IrType::F128 => {
                    // Load F128 value to x87 and store to stack
                    self.emit_f128_load_to_x87(arg);
                    emit!(self.state, "    fstpt {}(%esp)", offset);
                    offset += 12;
                }
                _ if is_i128_type(ty) => {
                    // Copy 16 bytes
                    if let Operand::Value(v) = arg {
                        if let Some(slot) = self.state.get_slot(v.0) {
                            for j in (0..16).step_by(4) {
                                emit!(self.state, "    movl {}(%ebp), %eax", slot.0 + j as i64);
                                emit!(self.state, "    movl %eax, {}(%esp)", offset + j as i64);
                            }
                        }
                    }
                    offset += 16;
                }
                _ => {
                    self.emit_load_operand(arg);
                    emit!(self.state, "    movl %eax, {}(%esp)", offset);
                    offset += 4;
                }
            }
        }

        // Phase 2: Load register args into ECX and EDX.
        // Load EDX first (arg 1) then ECX (arg 0), because loading arg 0
        // may clobber EDX if it involves function calls.
        if reg_count >= 2 {
            self.emit_load_operand(&args[1]);
            self.state.emit("    movl %eax, %edx");
        }
        if reg_count >= 1 {
            self.emit_load_operand(&args[0]);
            self.state.emit("    movl %eax, %ecx");
        }

        // Phase 3: Emit the call.
        if indirect {
            // Reload function pointer from spill slot
            let fptr_offset = stack_arg_space as i64;
            emit!(self.state, "    movl {}(%esp), %eax", fptr_offset);
            self.state.emit("    call *%eax");
        } else if let Some(name) = direct_name {
            emit!(self.state, "    call {}", name);
        }

        // Phase 4: For indirect calls, pop the spilled function pointer.
        // Note: callee already cleaned up the stack args, so we only need
        // to handle the fptr spill and alignment padding.
        if indirect {
            self.state.emit("    addl $4, %esp"); // pop fptr spill
        }
        // Clean up alignment padding (the difference between actual stack bytes and aligned)
        let padding = stack_arg_space - stack_bytes;
        if padding > 0 {
            emit!(self.state, "    addl ${}, %esp", padding);
        }

        // Phase 5: Store return value.
        if let Some(dest) = dest {
            self.emit_call_store_result(&dest, return_type);
        }

        self.state.reg_cache.invalidate_acc();
    }

    /// Copy `n_bytes` from stack slot to call stack area, 4 bytes at a time.
    pub(super) fn emit_copy_slot_to_stack(&mut self, slot_offset: i64, stack_offset: usize, n_bytes: usize) {
        let mut copied = 0usize;
        while copied + 4 <= n_bytes {
            emit!(self.state, "    movl {}(%ebp), %eax", slot_offset + copied as i64);
            emit!(self.state, "    movl %eax, {}(%esp)", stack_offset + copied);
            copied += 4;
        }
        while copied < n_bytes {
            emit!(self.state, "    movb {}(%ebp), %al", slot_offset + copied as i64);
            emit!(self.state, "    movb %al, {}(%esp)", stack_offset + copied);
            copied += 1;
        }
        self.state.reg_cache.invalidate_acc();
    }

    /// Fallback: store eax to stack, zero-fill remaining bytes.
    pub(super) fn emit_eax_to_stack_zeroed(&mut self, arg: &Operand, stack_offset: usize, total_bytes: usize) {
        self.operand_to_eax(arg);
        emit!(self.state, "    movl %eax, {}(%esp)", stack_offset);
        for j in (4..total_bytes).step_by(4) {
            emit!(self.state, "    movl $0, {}(%esp)", stack_offset + j);
        }
    }

    /// Emit I128 argument to call stack (16 bytes).
    pub(super) fn emit_call_i128_stack_arg(&mut self, arg: &Operand, stack_offset: usize) {
        if let Operand::Value(v) = arg {
            if let Some(slot) = self.state.get_slot(v.0) {
                self.emit_copy_slot_to_stack(slot.0, stack_offset, 16);
            } else {
                self.emit_eax_to_stack_zeroed(arg, stack_offset, 16);
            }
        }
    }

    /// Emit F128 (long double) argument to call stack (12 bytes).
    pub(super) fn emit_call_f128_stack_arg(&mut self, arg: &Operand, stack_offset: usize) {
        match arg {
            Operand::Value(v) => {
                if self.state.f128_direct_slots.contains(&v.0) {
                    if let Some(slot) = self.state.get_slot(v.0) {
                        emit!(self.state, "    fldt {}(%ebp)", slot.0);
                        emit!(self.state, "    fstpt {}(%esp)", stack_offset);
                    }
                } else if let Some(slot) = self.state.get_slot(v.0) {
                    self.emit_copy_slot_to_stack(slot.0, stack_offset, 12);
                } else {
                    self.emit_eax_to_stack_zeroed(arg, stack_offset, 12);
                }
            }
            Operand::Const(IrConst::LongDouble(_, bytes)) => {
                let x87 = crate::common::long_double::f128_bytes_to_x87_bytes(bytes);
                let dword0 = i32::from_le_bytes([x87[0], x87[1], x87[2], x87[3]]);
                let dword1 = i32::from_le_bytes([x87[4], x87[5], x87[6], x87[7]]);
                let word2 = i16::from_le_bytes([x87[8], x87[9]]) as i32;
                emit!(self.state, "    movl ${}, {}(%esp)", dword0, stack_offset);
                emit!(self.state, "    movl ${}, {}(%esp)", dword1, stack_offset + 4);
                emit!(self.state, "    movw ${}, {}(%esp)", word2, stack_offset + 8);
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
                emit!(self.state, "    fstpt {}(%esp)", stack_offset);
            }
            _ => {
                self.emit_eax_to_stack_zeroed(arg, stack_offset, 12);
            }
        }
    }

    /// Emit struct-by-value argument to call stack.
    pub(super) fn emit_call_struct_stack_arg(&mut self, arg: &Operand, stack_offset: usize, size: usize) {
        if let Operand::Value(v) = arg {
            if self.state.is_alloca(v.0) {
                if let Some(slot) = self.state.get_slot(v.0) {
                    self.emit_copy_slot_to_stack(slot.0, stack_offset, size);
                }
            } else {
                // Non-alloca: value is a pointer to struct data.
                self.operand_to_eax(arg);
                self.state.emit("    movl %eax, %ecx");
                let mut copied = 0usize;
                while copied + 4 <= size {
                    emit!(self.state, "    movl {}(%ecx), %eax", copied);
                    emit!(self.state, "    movl %eax, {}(%esp)", stack_offset + copied);
                    copied += 4;
                }
                while copied < size {
                    emit!(self.state, "    movb {}(%ecx), %al", copied);
                    emit!(self.state, "    movb %al, {}(%esp)", stack_offset + copied);
                    copied += 1;
                }
                self.state.reg_cache.invalidate_acc();
            }
        }
    }

    /// Emit 8-byte scalar (F64/I64/U64) to call stack.
    pub(super) fn emit_call_8byte_stack_arg(&mut self, arg: &Operand, ty: IrType, stack_offset: usize) {
        if let Operand::Value(v) = arg {
            if let Some(slot) = self.state.get_slot(v.0) {
                emit!(self.state, "    movl {}(%ebp), %eax", slot.0);
                emit!(self.state, "    movl %eax, {}(%esp)", stack_offset);
                emit!(self.state, "    movl {}(%ebp), %eax", slot.0 + 4);
                emit!(self.state, "    movl %eax, {}(%esp)", stack_offset + 4);
                self.state.reg_cache.invalidate_acc();
            } else {
                self.operand_to_eax(arg);
                emit!(self.state, "    movl %eax, {}(%esp)", stack_offset);
                emit!(self.state, "    movl $0, {}(%esp)", stack_offset + 4);
            }
        } else if ty == IrType::F64 {
            if let Operand::Const(IrConst::F64(f)) = arg {
                let bits = f.to_bits();
                let lo = (bits & 0xFFFF_FFFF) as u32;
                let hi = (bits >> 32) as u32;
                emit!(self.state, "    movl ${}, {}(%esp)", lo as i32, stack_offset);
                emit!(self.state, "    movl ${}, {}(%esp)", hi as i32, stack_offset + 4);
            } else {
                self.operand_to_eax(arg);
                emit!(self.state, "    movl %eax, {}(%esp)", stack_offset);
                emit!(self.state, "    movl $0, {}(%esp)", stack_offset + 4);
            }
        } else {
            // I64/U64 constant
            self.operand_to_eax(arg);
            emit!(self.state, "    movl %eax, {}(%esp)", stack_offset);
            if let Operand::Const(IrConst::I64(v)) = arg {
                let hi = ((*v as u64) >> 32) as i32;
                emit!(self.state, "    movl ${}, {}(%esp)", hi, stack_offset + 4);
            } else {
                emit!(self.state, "    movl $0, {}(%esp)", stack_offset + 4);
            }
        }
    }

}
