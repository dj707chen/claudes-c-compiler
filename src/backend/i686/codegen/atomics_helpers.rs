//! Wide (64-bit) atomic operation helpers for i686.
//!
//! On i686, 64-bit atomic operations require lock cmpxchg8b since the
//! architecture only supports 32-bit atomic instructions natively.

use crate::ir::ir::{AtomicRmwOp, Operand, Value};
use crate::common::types::IrType;
use crate::backend::traits::ArchCodegen;
use super::codegen::I686Codegen;
use crate::emit;

impl I686Codegen {
    /// Check if a type requires 64-bit atomic handling on i686 (needs cmpxchg8b).
    pub(super) fn is_atomic_wide(&self, ty: IrType) -> bool {
        matches!(ty, IrType::I64 | IrType::U64 | IrType::F64)
    }

    /// 64-bit atomic RMW using lock cmpxchg8b loop.
    ///
    /// cmpxchg8b compares edx:eax with 8 bytes at memory location.
    /// If equal, stores ecx:ebx to memory. If not, loads memory into edx:eax.
    /// We use a loop: load old value, compute new value, try cmpxchg8b.
    ///
    /// Register plan:
    ///   esi = pointer to atomic variable (saved/restored)
    ///   edx:eax = old (expected) value
    ///   ecx:ebx = new (desired) value
    ///   Stack: saved operand value (8 bytes)
    pub(super) fn emit_atomic_rmw_wide(&mut self, dest: &Value, op: AtomicRmwOp, ptr: &Operand,
                            val: &Operand) {
        // Save callee-saved registers we need to clobber
        self.state.emit("    pushl %ebx");
        self.state.emit("    pushl %esi");

        // Load pointer into esi
        self.operand_to_eax(ptr);
        self.state.emit("    movl %eax, %esi");

        // Load 64-bit operand value onto stack (8 bytes)
        self.emit_load_acc_pair(val);
        self.state.emit("    pushl %edx");  // high word at 4(%esp)
        self.state.emit("    pushl %eax");  // low word at (%esp)

        // Load current value from memory into edx:eax
        self.state.emit("    movl (%esi), %eax");
        self.state.emit("    movl 4(%esi), %edx");

        match op {
            AtomicRmwOp::Xchg => {
                // For exchange, the desired value is the operand (constant across retries)
                self.state.emit("    movl (%esp), %ebx");
                self.state.emit("    movl 4(%esp), %ecx");
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::Add => {
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    movl %eax, %ebx");
                self.state.emit("    movl %edx, %ecx");
                self.state.emit("    addl (%esp), %ebx");
                self.state.emit("    adcl 4(%esp), %ecx");
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::Sub => {
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    movl %eax, %ebx");
                self.state.emit("    movl %edx, %ecx");
                self.state.emit("    subl (%esp), %ebx");
                self.state.emit("    sbbl 4(%esp), %ecx");
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::And => {
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    movl %eax, %ebx");
                self.state.emit("    movl %edx, %ecx");
                self.state.emit("    andl (%esp), %ebx");
                self.state.emit("    andl 4(%esp), %ecx");
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::Or => {
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    movl %eax, %ebx");
                self.state.emit("    movl %edx, %ecx");
                self.state.emit("    orl (%esp), %ebx");
                self.state.emit("    orl 4(%esp), %ecx");
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::Xor => {
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    movl %eax, %ebx");
                self.state.emit("    movl %edx, %ecx");
                self.state.emit("    xorl (%esp), %ebx");
                self.state.emit("    xorl 4(%esp), %ecx");
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::Nand => {
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    movl %eax, %ebx");
                self.state.emit("    movl %edx, %ecx");
                self.state.emit("    andl (%esp), %ebx");
                self.state.emit("    andl 4(%esp), %ecx");
                self.state.emit("    notl %ebx");
                self.state.emit("    notl %ecx");
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
            AtomicRmwOp::TestAndSet => {
                // For 64-bit test-and-set, set the low byte to 1, rest to 0
                self.state.emit("    movl $1, %ebx");
                self.state.emit("    xorl %ecx, %ecx");
                let loop_label = format!(".Latomic_{}", self.state.next_label_id());
                emit!(self.state, "{}:", loop_label);
                self.state.emit("    lock cmpxchg8b (%esi)");
                emit!(self.state, "    jne {}", loop_label);
            }
        }

        // Clean up stack (remove 8-byte operand value)
        self.state.emit("    addl $8, %esp");
        // Restore callee-saved registers
        self.state.emit("    popl %esi");
        self.state.emit("    popl %ebx");

        // Result (old value) is in edx:eax — store to dest's 64-bit stack slot
        self.state.reg_cache.invalidate_acc();
        self.emit_store_acc_pair(dest);
    }

    /// 64-bit atomic compare-exchange using lock cmpxchg8b.
    ///
    /// cmpxchg8b: compares edx:eax with 8 bytes at memory.
    /// If equal, stores ecx:ebx to memory and sets ZF.
    /// If not equal, loads memory into edx:eax and clears ZF.
    pub(super) fn emit_atomic_cmpxchg_wide(&mut self, dest: &Value, ptr: &Operand, expected: &Operand,
                                desired: &Operand, returns_bool: bool) {
        // Save callee-saved registers
        self.state.emit("    pushl %ebx");
        self.state.emit("    pushl %esi");

        // Load pointer into esi
        self.operand_to_eax(ptr);
        self.state.emit("    movl %eax, %esi");

        // Load expected into edx:eax, save on stack temporarily
        self.emit_load_acc_pair(expected);
        self.state.emit("    pushl %edx");
        self.state.emit("    pushl %eax");

        // Load desired into ecx:ebx
        self.emit_load_acc_pair(desired);
        self.state.emit("    movl %eax, %ebx");
        self.state.emit("    movl %edx, %ecx");

        // Restore expected into edx:eax
        self.state.emit("    popl %eax");
        self.state.emit("    popl %edx");

        // Execute cmpxchg8b
        self.state.emit("    lock cmpxchg8b (%esi)");

        if returns_bool {
            self.state.emit("    sete %al");
            self.state.emit("    movzbl %al, %eax");
            // Restore callee-saved registers
            self.state.emit("    popl %esi");
            self.state.emit("    popl %ebx");
            self.state.reg_cache.invalidate_acc();
            self.store_eax_to(dest);
        } else {
            // Result (old value) is in edx:eax
            // Restore callee-saved registers
            self.state.emit("    popl %esi");
            self.state.emit("    popl %ebx");
            self.state.reg_cache.invalidate_acc();
            self.emit_store_acc_pair(dest);
        }
    }

    /// 64-bit atomic load using cmpxchg8b with expected == desired == 0.
    ///
    /// cmpxchg8b always loads the current memory value into edx:eax on failure,
    /// so we set edx:eax = ecx:ebx = 0 and execute cmpxchg8b. If the memory
    /// happens to be 0, the exchange writes 0 (no change). If non-zero,
    /// we get the current value in edx:eax without modifying memory.
    pub(super) fn emit_atomic_load_wide(&mut self, dest: &Value, ptr: &Operand) {
        self.state.emit("    pushl %ebx");
        self.state.emit("    pushl %esi");

        self.operand_to_eax(ptr);
        self.state.emit("    movl %eax, %esi");

        // Set all registers to zero: edx:eax = ecx:ebx = 0
        self.state.emit("    xorl %eax, %eax");
        self.state.emit("    xorl %edx, %edx");
        self.state.emit("    xorl %ebx, %ebx");
        self.state.emit("    xorl %ecx, %ecx");
        // lock cmpxchg8b: if (%esi) == 0 -> store 0 (no change), else load into edx:eax
        self.state.emit("    lock cmpxchg8b (%esi)");

        self.state.emit("    popl %esi");
        self.state.emit("    popl %ebx");

        self.state.reg_cache.invalidate_acc();
        self.emit_store_acc_pair(dest);
    }

    /// 64-bit atomic store using a cmpxchg8b loop.
    ///
    /// There is no single instruction for atomic 64-bit stores on i686, so we
    /// use a cmpxchg8b loop: read current value, try to replace with desired.
    pub(super) fn emit_atomic_store_wide(&mut self, ptr: &Operand, val: &Operand) {
        self.state.emit("    pushl %ebx");
        self.state.emit("    pushl %esi");

        // Load pointer into esi
        self.operand_to_eax(ptr);
        self.state.emit("    movl %eax, %esi");

        // Load desired value into ecx:ebx
        self.emit_load_acc_pair(val);
        self.state.emit("    movl %eax, %ebx");
        self.state.emit("    movl %edx, %ecx");

        // Load current value from memory into edx:eax (initial guess for cmpxchg8b)
        self.state.emit("    movl (%esi), %eax");
        self.state.emit("    movl 4(%esi), %edx");

        let loop_label = format!(".Latomic_{}", self.state.next_label_id());
        emit!(self.state, "{}:", loop_label);
        self.state.emit("    lock cmpxchg8b (%esi)");
        emit!(self.state, "    jne {}", loop_label);

        self.state.emit("    popl %esi");
        self.state.emit("    popl %ebx");
        self.state.reg_cache.invalidate_acc();
    }

}
