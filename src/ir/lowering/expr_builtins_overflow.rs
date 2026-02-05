//! Overflow-checking arithmetic builtins for the lowerer.
//!
//! Handles __builtin_{s,u}{add,sub,mul}{,l,ll}_overflow and the generic
//! __builtin_{add,sub,mul}_overflow variants. These builtins perform an
//! arithmetic operation, store the result through a pointer, and return
//! 1 (bool true) if the operation overflowed.
//!
//! Also handles __builtin_{add,sub,mul}_overflow_p(a, b, (T)0) predicate-only
//! variants (GCC 7+) which return 1 if the operation would overflow type T,
//! without storing the result.

use crate::frontend::parser::ast::{Expr};
use crate::ir::reexports::{
    Instruction,
    IrBinOp,
    IrCmpOp,
    IrConst,
    Operand,
    Value,
};
use crate::common::types::{AddressSpace, IrType, CType, target_int_ir_type, target_is_32bit};
use super::lower::Lowerer;

impl Lowerer {
    /// Lower __builtin_{add,sub,mul}_overflow(a, b, result_ptr) and type-specific variants.
    ///
    /// These builtins perform an arithmetic operation, store the result through
    /// the pointer, and return 1 (bool true) if the operation overflowed.
    pub(super) fn lower_overflow_builtin(&mut self, name: &str, args: &[Expr], op: IrBinOp) -> Option<Operand> {
        if args.len() < 3 {
            return Some(Operand::Const(IrConst::I64(0)));
        }

        // Determine the result type from the builtin name or from the pointer argument type.
        // Type-specific variants: __builtin_sadd_overflow (signed int), __builtin_uaddll_overflow (unsigned long long)
        // Generic variants: __builtin_add_overflow - type from pointer arg
        let (result_ir_ty, is_signed) = self.overflow_result_type(name, &args[2]);

        // Detect generic variants: __builtin_{add,sub,mul}_overflow
        // (as opposed to type-specific like __builtin_sadd_overflow, __builtin_uaddll_overflow)
        let is_generic = name == "__builtin_add_overflow"
            || name == "__builtin_sub_overflow"
            || name == "__builtin_mul_overflow";

        // Lower the two operands
        let lhs_raw = self.lower_expr(&args[0]);
        let rhs_raw = self.lower_expr(&args[1]);

        // For generic variants, operands may have different types than the result.
        // We need to perform the computation in a type wide enough to hold all
        // possible values from both operand types, then check if the result fits
        // in the target result type.
        let lhs_src_ctype = self.expr_ctype(&args[0]);
        let rhs_src_ctype = self.expr_ctype(&args[1]);

        // Get the result pointer
        let result_ptr_val = self.lower_expr(&args[2]);
        let result_ptr = self.operand_to_value(result_ptr_val);

        if is_generic {
            let lhs_src_ir = IrType::from_ctype(&lhs_src_ctype);
            let rhs_src_ir = IrType::from_ctype(&rhs_src_ctype);

            // Determine a computation type wide enough to hold the exact mathematical
            // result. We need a signed type that can represent all values of both
            // operands without loss.
            let max_src_size = lhs_src_ir.size().max(rhs_src_ir.size()).max(result_ir_ty.size());
            // If any source is unsigned and as wide as max_src_size, we need one
            // extra width level to avoid losing the unsigned range.
            let needs_wider = (!lhs_src_ctype.is_signed() && lhs_src_ir.size() >= max_src_size)
                || (!rhs_src_ctype.is_signed() && rhs_src_ir.size() >= max_src_size);
            let compute_size = if needs_wider { max_src_size * 2 } else { max_src_size };
            let compute_size = compute_size.max(result_ir_ty.size());
            // Use a signed compute type so we can detect negative results
            let compute_ty = match compute_size {
                1 => IrType::I16, // widen 8-bit to at least 16
                2 => IrType::I32,
                4 => IrType::I64,
                8 => IrType::I64,
                _ => IrType::I128,
            };

            // Cast operands to the compute type respecting their signedness
            let lhs_compute = if lhs_src_ir != compute_ty {
                Operand::Value(self.emit_cast_val(lhs_raw, lhs_src_ir, compute_ty))
            } else {
                lhs_raw
            };
            let rhs_compute = if rhs_src_ir != compute_ty {
                Operand::Value(self.emit_cast_val(rhs_raw, rhs_src_ir, compute_ty))
            } else {
                rhs_raw
            };

            if compute_ty.size() > result_ir_ty.size() {
                // We were able to widen: compute exactly, truncate, and check
                // if the truncated result round-trips back to the wide value.
                let wide_result = self.emit_binop_val(op, lhs_compute, rhs_compute, compute_ty);

                let truncated = self.emit_cast_val(Operand::Value(wide_result), compute_ty, result_ir_ty);

                let extended_back = self.emit_cast_val(Operand::Value(truncated), result_ir_ty, compute_ty);
                let overflow = self.emit_cmp_val(
                    IrCmpOp::Ne,
                    Operand::Value(wide_result),
                    Operand::Value(extended_back),
                    compute_ty,
                );

                // Store the truncated result
                self.emit(Instruction::Store { val: Operand::Value(truncated), ptr: result_ptr, ty: result_ir_ty,
                 seg_override: AddressSpace::Default });

                Some(Operand::Value(overflow))
            } else if compute_ty != result_ir_ty && compute_ty.size() == result_ir_ty.size() {
                // Same bit width but different signedness (e.g. compute in I64, result is U64).
                // The round-trip cast won't detect overflow because the bit pattern is preserved.
                // We need to check signedness constraints explicitly.
                let wide_result = self.emit_binop_val(op, lhs_compute, rhs_compute, compute_ty);

                // Store the result (reinterpreted as the result type)
                let truncated = self.emit_cast_val(Operand::Value(wide_result), compute_ty, result_ir_ty);
                self.emit(Instruction::Store { val: Operand::Value(truncated), ptr: result_ptr, ty: result_ir_ty,
                 seg_override: AddressSpace::Default });

                let overflow = if compute_ty.is_signed() && !result_ir_ty.is_signed() {
                    // Computed in signed type, result is unsigned.
                    // Overflow if the mathematical result is negative (can't fit in unsigned).
                    let bits = (compute_ty.size() * 8) as i64;
                    let shifted = self.emit_binop_val(IrBinOp::AShr, Operand::Value(wide_result),
                        Operand::Const(IrConst::I64(bits - 1)), compute_ty);
                    // shifted is all-ones (-1) if negative, all-zeros (0) if non-negative
                    self.emit_binop_val(IrBinOp::And, Operand::Value(shifted),
                        Operand::Const(IrConst::I64(1)), compute_ty)
                } else {
                    // Computed in unsigned type, result is signed.
                    // Overflow if the value exceeds SIGNED_MAX (i.e. sign bit is set).
                    let bits = (compute_ty.size() * 8) as i64;
                    let shifted = self.emit_binop_val(IrBinOp::LShr, Operand::Value(wide_result),
                        Operand::Const(IrConst::I64(bits - 1)), compute_ty);
                    self.emit_binop_val(IrBinOp::And, Operand::Value(shifted),
                        Operand::Const(IrConst::I64(1)), compute_ty)
                };

                Some(Operand::Value(overflow))
            } else {
                // Could not widen (e.g. both operands and result are int64).
                // Fall back to same-type overflow detection.
                let result = self.emit_binop_val(op, lhs_compute, rhs_compute, result_ir_ty);

                let overflow = if is_signed {
                    self.compute_signed_overflow(op, lhs_compute, rhs_compute, result, result_ir_ty)
                } else {
                    self.compute_unsigned_overflow(op, lhs_compute, rhs_compute, result, result_ir_ty)
                };

                self.emit(Instruction::Store { val: Operand::Value(result), ptr: result_ptr, ty: result_ir_ty,
                 seg_override: AddressSpace::Default });

                Some(Operand::Value(overflow))
            }
        } else {
            // Type-specific variants: operands already match the result type
            let (lhs_val, rhs_val) = (lhs_raw, rhs_raw);

            // Perform the operation in the result type
            let result = self.emit_binop_val(op, lhs_val, rhs_val, result_ir_ty);

            // Compute the overflow flag BEFORE storing the result, because the
            // output pointer may alias an input.
            let overflow = if is_signed {
                self.compute_signed_overflow(op, lhs_val, rhs_val, result, result_ir_ty)
            } else {
                self.compute_unsigned_overflow(op, lhs_val, rhs_val, result, result_ir_ty)
            };

            // Store the result
            self.emit(Instruction::Store { val: Operand::Value(result), ptr: result_ptr, ty: result_ir_ty,
             seg_override: AddressSpace::Default });

            Some(Operand::Value(overflow))
        }
    }

    /// Lower __builtin_{add,sub,mul}_overflow_p(a, b, (T)0) predicate-only variants.
    ///
    /// These return 1 if the operation would overflow type T, 0 otherwise.
    /// The third argument is a value expression whose type determines T (the value itself is ignored).
    pub(super) fn lower_overflow_p_builtin(&mut self, args: &[Expr], op: IrBinOp) -> Option<Operand> {
        if args.len() < 3 {
            return Some(Operand::Const(IrConst::I64(0)));
        }

        // The third argument determines the result type (only its type matters, not its value).
        let result_ctype = self.expr_ctype(&args[2]);
        let result_ir_ty = IrType::from_ctype(&result_ctype);

        // Lower the two operands
        let lhs_raw = self.lower_expr(&args[0]);
        let rhs_raw = self.lower_expr(&args[1]);

        // Lower but discard the third argument (it may have side effects, though
        // in practice it's always a cast of 0)
        let _ = self.lower_expr(&args[2]);

        // Use the same wider-type computation as the non-_p generic variant:
        // compute in a type wide enough to hold the exact result, then check
        // if truncating to the result type loses information.
        let lhs_src_ctype = self.expr_ctype(&args[0]);
        let rhs_src_ctype = self.expr_ctype(&args[1]);
        let lhs_src_ir = IrType::from_ctype(&lhs_src_ctype);
        let rhs_src_ir = IrType::from_ctype(&rhs_src_ctype);

        let max_src_size = lhs_src_ir.size().max(rhs_src_ir.size()).max(result_ir_ty.size());
        let needs_wider = (!lhs_src_ctype.is_signed() && lhs_src_ir.size() >= max_src_size)
            || (!rhs_src_ctype.is_signed() && rhs_src_ir.size() >= max_src_size);
        let compute_size = if needs_wider { max_src_size * 2 } else { max_src_size };
        let compute_size = compute_size.max(result_ir_ty.size());
        let compute_ty = match compute_size {
            1 => IrType::I16,
            2 => IrType::I32,
            4 => IrType::I64,
            8 => IrType::I64,
            _ => IrType::I128,
        };

        let lhs_compute = if lhs_src_ir != compute_ty {
            Operand::Value(self.emit_cast_val(lhs_raw, lhs_src_ir, compute_ty))
        } else {
            lhs_raw
        };
        let rhs_compute = if rhs_src_ir != compute_ty {
            Operand::Value(self.emit_cast_val(rhs_raw, rhs_src_ir, compute_ty))
        } else {
            rhs_raw
        };

        if compute_ty.size() > result_ir_ty.size() {
            // We were able to widen: compute exactly, truncate, and check
            // if the truncated result round-trips back to the wide value.
            let wide_result = self.emit_binop_val(op, lhs_compute, rhs_compute, compute_ty);

            let truncated = self.emit_cast_val(Operand::Value(wide_result), compute_ty, result_ir_ty);

            let extended_back = self.emit_cast_val(Operand::Value(truncated), result_ir_ty, compute_ty);
            let overflow = self.emit_cmp_val(
                IrCmpOp::Ne,
                Operand::Value(wide_result),
                Operand::Value(extended_back),
                compute_ty,
            );

            Some(Operand::Value(overflow))
        } else if compute_ty != result_ir_ty && compute_ty.size() == result_ir_ty.size() {
            // Same bit width but different signedness (e.g. compute in I64, result is U64).
            // The round-trip cast won't detect overflow because the bit pattern is preserved.
            let wide_result = self.emit_binop_val(op, lhs_compute, rhs_compute, compute_ty);

            let overflow = if compute_ty.is_signed() && !result_ir_ty.is_signed() {
                // Computed in signed type, result is unsigned: overflow if negative
                let bits = (compute_ty.size() * 8) as i64;
                let shifted = self.emit_binop_val(IrBinOp::AShr, Operand::Value(wide_result),
                    Operand::Const(IrConst::I64(bits - 1)), compute_ty);
                self.emit_binop_val(IrBinOp::And, Operand::Value(shifted),
                    Operand::Const(IrConst::I64(1)), compute_ty)
            } else {
                // Computed in unsigned type, result is signed: overflow if sign bit set
                let bits = (compute_ty.size() * 8) as i64;
                let shifted = self.emit_binop_val(IrBinOp::LShr, Operand::Value(wide_result),
                    Operand::Const(IrConst::I64(bits - 1)), compute_ty);
                self.emit_binop_val(IrBinOp::And, Operand::Value(shifted),
                    Operand::Const(IrConst::I64(1)), compute_ty)
            };

            Some(Operand::Value(overflow))
        } else {
            // Could not widen (e.g. both operands and result are int64).
            // Fall back to same-type overflow detection.
            let is_signed = result_ctype.is_signed();
            let result = self.emit_binop_val(op, lhs_compute, rhs_compute, result_ir_ty);

            let overflow = if is_signed {
                self.compute_signed_overflow(op, lhs_compute, rhs_compute, result, result_ir_ty)
            } else {
                self.compute_unsigned_overflow(op, lhs_compute, rhs_compute, result, result_ir_ty)
            };

            Some(Operand::Value(overflow))
        }
    }

    /// Determine the result IrType and signedness for an overflow builtin.
    fn overflow_result_type(&self, name: &str, result_ptr_expr: &Expr) -> (IrType, bool) {
        // Check type-specific variants first
        // __builtin_sadd_overflow -> signed int
        // __builtin_saddl_overflow -> signed long
        // __builtin_saddll_overflow -> signed long long
        // __builtin_uadd_overflow -> unsigned int
        // etc.
        if name.starts_with("__builtin_s") && name != "__builtin_sub_overflow" {
            // Signed type-specific variant
            let is_signed = true;
            let ty = if name.ends_with("ll_overflow") {
                IrType::I64
            } else if name.ends_with("l_overflow") {
                target_int_ir_type() // long: I64 on LP64, I32 on ILP32
            } else {
                IrType::I32
            };
            return (ty, is_signed);
        }
        if name.starts_with("__builtin_u") {
            // Unsigned type-specific variant
            let is_signed = false;
            let ty = if name.ends_with("ll_overflow") {
                IrType::U64
            } else if name.ends_with("l_overflow") {
                if target_is_32bit() { IrType::U32 } else { IrType::U64 } // unsigned long
            } else {
                IrType::U32
            };
            return (ty, is_signed);
        }

        // Generic variant: determine type from the pointer argument's pointee type
        let result_ctype = self.expr_ctype(result_ptr_expr);
        if let CType::Pointer(pointee, _) = &result_ctype {
            let is_signed = pointee.is_signed();
            let ir_ty = IrType::from_ctype(pointee);
            return (ir_ty, is_signed);
        }

        // If we can't determine the type (e.g., &local), try to get the
        // pointee from the expression structure
        if let Expr::AddressOf(inner, _) = result_ptr_expr {
            let inner_ctype = self.expr_ctype(inner);
            let is_signed = inner_ctype.is_signed();
            let ir_ty = IrType::from_ctype(&inner_ctype);
            return (ir_ty, is_signed);
        }

        // Default to signed long if we can't determine
        (target_int_ir_type(), true)
    }

    /// Compute signed overflow flag for add/sub/mul.
    fn compute_signed_overflow(
        &mut self,
        op: IrBinOp,
        lhs: Operand,
        rhs: Operand,
        result: Value,
        ty: IrType,
    ) -> Value {
        let bits = (ty.size() * 8) as i64;
        match op {
            IrBinOp::Add => {
                // Signed add overflow: ((result ^ lhs) & (result ^ rhs)) < 0
                // i.e., overflow if both operands have same sign and result has different sign
                let xor_lhs = self.emit_binop_val(IrBinOp::Xor, Operand::Value(result), lhs, ty);
                let xor_rhs = self.emit_binop_val(IrBinOp::Xor, Operand::Value(result), rhs, ty);
                let and_val = self.emit_binop_val(IrBinOp::And, Operand::Value(xor_lhs), Operand::Value(xor_rhs), ty);
                // Check sign bit: shift right by (bits-1) and check if non-zero
                let shifted = self.emit_binop_val(IrBinOp::AShr, Operand::Value(and_val), Operand::Const(IrConst::I64(bits - 1)), ty);
                // The shifted value is all-ones (-1) if overflow, all-zeros (0) if not
                // Convert to 0/1 by AND with 1
                self.emit_binop_val(IrBinOp::And, Operand::Value(shifted), Operand::Const(IrConst::I64(1)), ty)
            }
            IrBinOp::Sub => {
                // Signed sub overflow: ((lhs ^ rhs) & (result ^ lhs)) < 0
                // overflow if operands have different signs and result sign differs from lhs
                let xor_ops = self.emit_binop_val(IrBinOp::Xor, lhs, rhs, ty);
                let xor_res = self.emit_binop_val(IrBinOp::Xor, Operand::Value(result), lhs, ty);
                let and_val = self.emit_binop_val(IrBinOp::And, Operand::Value(xor_ops), Operand::Value(xor_res), ty);
                let shifted = self.emit_binop_val(IrBinOp::AShr, Operand::Value(and_val), Operand::Const(IrConst::I64(bits - 1)), ty);
                self.emit_binop_val(IrBinOp::And, Operand::Value(shifted), Operand::Const(IrConst::I64(1)), ty)
            }
            IrBinOp::Mul => {
                // For signed multiply overflow, we widen to double width, multiply, then
                // check if the result fits in the original width by checking that
                // sign-extending the truncated result gives back the full result.
                let wide_ty = Self::double_width_type(ty, true);
                let lhs_wide = self.emit_cast_val(lhs, ty, wide_ty);
                let rhs_wide = self.emit_cast_val(rhs, ty, wide_ty);
                let wide_result = self.emit_binop_val(IrBinOp::Mul, Operand::Value(lhs_wide), Operand::Value(rhs_wide), wide_ty);
                // Truncate back to original width
                let truncated = self.emit_cast_val(Operand::Value(wide_result), wide_ty, ty);
                // Sign-extend truncated back to wide type
                let sign_extended = self.emit_cast_val(Operand::Value(truncated), ty, wide_ty);
                // Overflow if wide_result != sign_extended
                self.emit_cmp_val(IrCmpOp::Ne, Operand::Value(wide_result), Operand::Value(sign_extended), wide_ty)
            }
            _ => unreachable!("overflow only for add/sub/mul"),
        }
    }

    /// Compute unsigned overflow flag for add/sub/mul.
    fn compute_unsigned_overflow(
        &mut self,
        op: IrBinOp,
        lhs: Operand,
        rhs: Operand,
        result: Value,
        ty: IrType,
    ) -> Value {
        // Make sure we're comparing in unsigned type
        let unsigned_ty = ty.to_unsigned();
        match op {
            IrBinOp::Add => {
                // Unsigned add overflow: result < lhs (wrapping means the sum is smaller)
                self.emit_cmp_val(IrCmpOp::Ult, Operand::Value(result), lhs, unsigned_ty)
            }
            IrBinOp::Sub => {
                // Unsigned sub overflow: lhs < rhs (borrow occurred)
                self.emit_cmp_val(IrCmpOp::Ult, lhs, rhs, unsigned_ty)
            }
            IrBinOp::Mul => {
                // Unsigned multiply overflow: widen to double width, multiply, check upper half is zero
                let wide_ty = Self::double_width_type(ty, false);
                let lhs_wide = self.emit_cast_val(lhs, unsigned_ty, wide_ty);
                let rhs_wide = self.emit_cast_val(rhs, unsigned_ty, wide_ty);
                let wide_result = self.emit_binop_val(IrBinOp::Mul, Operand::Value(lhs_wide), Operand::Value(rhs_wide), wide_ty);
                // Truncate back to original width
                let truncated = self.emit_cast_val(Operand::Value(wide_result), wide_ty, unsigned_ty);
                // Zero-extend truncated back to wide type
                let zero_extended = self.emit_cast_val(Operand::Value(truncated), unsigned_ty, wide_ty);
                // Overflow if wide_result != zero_extended
                self.emit_cmp_val(IrCmpOp::Ne, Operand::Value(wide_result), Operand::Value(zero_extended), wide_ty)
            }
            _ => unreachable!("overflow only for add/sub/mul"),
        }
    }

    /// Get the double-width integer type for overflow multiply detection.
    fn double_width_type(ty: IrType, signed: bool) -> IrType {
        match ty {
            IrType::I8 | IrType::U8 => if signed { IrType::I16 } else { IrType::U16 },
            IrType::I16 | IrType::U16 => if signed { IrType::I32 } else { IrType::U32 },
            IrType::I32 | IrType::U32 => if signed { IrType::I64 } else { IrType::U64 },
            IrType::I64 | IrType::U64 => if signed { IrType::I128 } else { IrType::U128 },
            // For 128-bit, we can't widen further; use a different strategy
            // (but this is extremely rare in practice)
            _ => if signed { IrType::I128 } else { IrType::U128 },
        }
    }
}
