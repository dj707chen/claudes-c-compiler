//! 64-bit integer operation helpers for i686.
//!
//! On i686, 64-bit operations (CLZ, CTZ, popcount, bswap) require
//! pair-based implementations operating on the high and low 32-bit halves.

use super::codegen::I686Codegen;
use crate::emit;

impl I686Codegen {
    /// clzll(x): Count leading zeros of 64-bit value in eax:edx.
    /// If high half (edx) != 0, result = lzcnt(edx).
    /// Otherwise, result = 32 + lzcnt(eax).
    pub(super) fn emit_i64_clz(&mut self) {
        let done = self.state.fresh_label("clz64_done");
        let hi_zero = self.state.fresh_label("clz64_hi_zero");
        // Test high half
        self.state.emit("    testl %edx, %edx");
        emit!(self.state, "    je {}", hi_zero);
        // High half is non-zero: result = lzcnt(edx)
        self.state.emit("    lzcntl %edx, %eax");
        self.state.emit("    xorl %edx, %edx");
        emit!(self.state, "    jmp {}", done);
        // High half is zero: result = 32 + lzcnt(eax)
        emit!(self.state, "{}:", hi_zero);
        self.state.emit("    lzcntl %eax, %eax");
        self.state.emit("    addl $32, %eax");
        self.state.emit("    xorl %edx, %edx");
        emit!(self.state, "{}:", done);
    }

    /// ctzll(x): Count trailing zeros of 64-bit value in eax:edx.
    /// If low half (eax) != 0, result = tzcnt(eax).
    /// Otherwise, result = 32 + tzcnt(edx).
    pub(super) fn emit_i64_ctz(&mut self) {
        let done = self.state.fresh_label("ctz64_done");
        let lo_zero = self.state.fresh_label("ctz64_lo_zero");
        // Test low half
        self.state.emit("    testl %eax, %eax");
        emit!(self.state, "    je {}", lo_zero);
        // Low half is non-zero: result = tzcnt(eax)
        self.state.emit("    tzcntl %eax, %eax");
        self.state.emit("    xorl %edx, %edx");
        emit!(self.state, "    jmp {}", done);
        // Low half is zero: result = 32 + tzcnt(edx)
        emit!(self.state, "{}:", lo_zero);
        self.state.emit("    tzcntl %edx, %eax");
        self.state.emit("    addl $32, %eax");
        self.state.emit("    xorl %edx, %edx");
        emit!(self.state, "{}:", done);
    }

    /// popcountll(x): Population count of 64-bit value in eax:edx.
    /// result = popcount(eax) + popcount(edx)
    pub(super) fn emit_i64_popcount(&mut self) {
        self.state.emit("    popcntl %edx, %ecx");
        self.state.emit("    popcntl %eax, %eax");
        self.state.emit("    addl %ecx, %eax");
        self.state.emit("    xorl %edx, %edx");
    }

    /// bswap64(x): Byte-swap 64-bit value in eax:edx.
    /// result_lo = bswap(high), result_hi = bswap(low)
    pub(super) fn emit_i64_bswap(&mut self) {
        // eax=low, edx=high
        // bswap each half, then swap: new_eax = bswap(edx), new_edx = bswap(eax)
        self.state.emit("    bswapl %eax");
        self.state.emit("    bswapl %edx");
        self.state.emit("    xchgl %eax, %edx");
    }

}
