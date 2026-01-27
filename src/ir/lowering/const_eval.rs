/// Constant expression evaluation for compile-time computation.
///
/// Core constant-folding logic: integer and floating-point arithmetic, cast
/// chains, sizeof/offsetof evaluation, and the top-level `eval_const_expr`
/// entry point. Global-address resolution lives in `const_eval_global_addr`
/// and initializer-list size computation in `const_eval_init_size`.

use crate::frontend::parser::ast::*;
use crate::ir::ir::*;
use crate::common::types::{CType, IrType};
use crate::common::const_arith;
use super::lowering::Lowerer;

impl Lowerer {
    /// Look up a pre-computed constant value from sema's ConstMap.
    /// Returns Some(IrConst) if sema successfully evaluated this expression
    /// at compile time during its pass.
    fn lookup_sema_const(&self, expr: &Expr) -> Option<IrConst> {
        let key = expr as *const Expr as usize;
        self.sema_const_values.get(&key).cloned()
    }

    /// Check if an expression tree contains a Sizeof node at any depth.
    /// Used to avoid trusting sema's pre-computed values for expressions involving
    /// sizeof, since sema evaluates sizeof before the lowerer resolves unsized array
    /// dimensions from initializers. For example, `sizeof(cases) / sizeof(cases[0]) + 1`
    /// contains Sizeof nodes in the division subtree, so the entire expression must
    /// be recomputed by the lowerer.
    fn expr_contains_sizeof(expr: &Expr) -> bool {
        match expr {
            Expr::Sizeof(_, _) => true,
            Expr::BinaryOp(_, lhs, rhs, _) => {
                Self::expr_contains_sizeof(lhs) || Self::expr_contains_sizeof(rhs)
            }
            Expr::UnaryOp(_, inner, _) | Expr::PostfixOp(_, inner, _) => {
                Self::expr_contains_sizeof(inner)
            }
            Expr::Cast(_, inner, _) => Self::expr_contains_sizeof(inner),
            Expr::Conditional(cond, then_e, else_e, _) => {
                Self::expr_contains_sizeof(cond)
                    || Self::expr_contains_sizeof(then_e)
                    || Self::expr_contains_sizeof(else_e)
            }
            Expr::GnuConditional(cond, else_e, _) => {
                Self::expr_contains_sizeof(cond) || Self::expr_contains_sizeof(else_e)
            }
            Expr::Comma(lhs, rhs, _) => {
                Self::expr_contains_sizeof(lhs) || Self::expr_contains_sizeof(rhs)
            }
            _ => false,
        }
    }

    /// Try to evaluate a constant expression at compile time.
    ///
    /// First checks sema's pre-computed ConstMap (O(1) lookup for expressions
    /// that sema could evaluate). Falls back to the lowerer's own evaluation
    /// for expressions that require lowering-specific state (global addresses,
    /// const local values, etc.).
    pub(super) fn eval_const_expr(&self, expr: &Expr) -> Option<IrConst> {
        // Fast path: consult sema's pre-computed constant values.
        // This avoids re-evaluating expressions that sema already handled.
        // We skip the sema lookup for:
        //   - Identifiers: the lowerer may have more information (const local values,
        //     static locals) that sema lacks.
        //   - Expressions containing sizeof: sema may have computed sizeof for unsized
        //     arrays (e.g., `PT cases[] = {1,2,3,...}`) before the lowerer resolved the
        //     actual element count from the initializer. The lowerer's sizeof_expr/
        //     sizeof_type uses the correctly-sized global, so we must recompute.
        //     This check covers sizeof itself and any parent expression containing it,
        //     such as `sizeof(x) / sizeof(x[0]) + 1`.
        if !matches!(expr, Expr::Identifier(_, _)) && !Self::expr_contains_sizeof(expr) {
            if let Some(val) = self.lookup_sema_const(expr) {
                return Some(val);
            }
        }
        match expr {
            // Preserve C type width: IntLiteral is `int` (32-bit) when value fits, otherwise `long`.
            Expr::IntLiteral(val, _) => {
                if *val >= i32::MIN as i64 && *val <= i32::MAX as i64 {
                    Some(IrConst::I32(*val as i32))
                } else {
                    Some(IrConst::I64(*val))
                }
            }
            Expr::LongLiteral(val, _) => {
                Some(IrConst::I64(*val))
            }
            // UIntLiteral stays as I64 to preserve the unsigned value.
            Expr::UIntLiteral(val, _) => {
                Some(IrConst::I64(*val as i64))
            }
            Expr::ULongLiteral(val, _) => {
                Some(IrConst::I64(*val as i64))
            }
            Expr::CharLiteral(ch, _) => {
                Some(IrConst::I32(*ch as i32))
            }
            Expr::FloatLiteral(val, _) => {
                Some(IrConst::F64(*val))
            }
            Expr::FloatLiteralF32(val, _) => {
                Some(IrConst::F32(*val as f32))
            }
            Expr::FloatLiteralLongDouble(val, bytes, _) => {
                Some(IrConst::long_double_with_bytes(*val, *bytes))
            }
            Expr::UnaryOp(UnaryOp::Plus, inner, _) => {
                self.eval_const_expr(inner)
            }
            Expr::UnaryOp(UnaryOp::Neg, inner, _) => {
                let val = self.eval_const_expr(inner)?;
                // C integer promotion: promote sub-int types to int before negation.
                // For unsigned sub-int types (unsigned char/short), zero-extend to
                // preserve the unsigned value. Without this, I8(-1) representing
                // unsigned char 255 would be negated as -(-1) = 1 instead of -(255) = -255.
                let promoted = self.promote_const_for_unary(inner, val);
                const_arith::negate_const(promoted)
            }
            Expr::BinaryOp(op, lhs, rhs, _) => {
                let l = self.eval_const_expr(lhs);
                let r = self.eval_const_expr(rhs);
                if let (Some(l), Some(r)) = (l, r) {
                    // Use infer_expr_type (C semantic types) for proper usual arithmetic
                    // conversions. get_expr_type returns IR storage types (IntLiteral → I64)
                    // which loses 32-bit width info needed for correct folding of
                    // expressions like (1 << 31) / N.
                    let lhs_ty = self.infer_expr_type(lhs);
                    let rhs_ty = self.infer_expr_type(rhs);
                    let result = self.eval_const_binop(op, &l, &r, lhs_ty, rhs_ty);
                    if result.is_some() {
                        return result;
                    }
                }
                // For subtraction, try evaluating as pointer difference:
                // &arr[5] - &arr[0], (char*)&s.c - (char*)&s.a, etc.
                // Both operands may be global address expressions referring to the
                // same symbol; the result is the byte offset difference (possibly
                // divided by the pointed-to element size for typed pointer subtraction).
                if *op == BinOp::Sub {
                    return self.eval_const_ptr_diff(lhs, rhs);
                }
                None
            }
            Expr::UnaryOp(UnaryOp::BitNot, inner, _) => {
                let val = self.eval_const_expr(inner)?;
                let promoted = self.promote_const_for_unary(inner, val);
                const_arith::bitnot_const(promoted)
            }
            Expr::Cast(ref target_type, inner, _) => {
                let target_ir_ty = self.type_spec_to_ir(target_type);
                let src_val = self.eval_const_expr(inner)?;

                // Handle float source types: use value-based conversion, not bit manipulation
                // For LongDouble, use full x87 precision to avoid losing mantissa bits
                if let IrConst::LongDouble(fv, bytes) = &src_val {
                    return IrConst::cast_long_double_to_target(*fv, bytes, target_ir_ty);
                }
                if let Some(fv) = src_val.to_f64() {
                    if matches!(&src_val, IrConst::F32(_) | IrConst::F64(_)) {
                        return IrConst::cast_float_to_target(fv, target_ir_ty);
                    }
                }

                // Handle I128 source: use full 128-bit value to avoid truncation
                // through the u64-based eval_const_expr_as_bits path
                if let IrConst::I128(v128) = src_val {
                    // Determine source signedness for int-to-float conversions
                    let src_ty = self.get_expr_type(inner);
                    let src_unsigned = src_ty.is_unsigned();
                    return Some(Self::cast_i128_to_ir_type(v128, target_ir_ty, src_unsigned));
                }

                // Integer source: use bit-based cast chain evaluation
                let (bits, _src_signed) = self.eval_const_expr_as_bits(inner)?;
                let target_width = target_ir_ty.size() * 8;
                let target_signed = matches!(target_ir_ty, IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::I128);

                // For 128-bit targets, sign-extend or zero-extend from 64 bits
                // based on the source expression's signedness (not the target's).
                // E.g., (unsigned __int128)(-1) should be all-ones (sign-extend signed source),
                // but (unsigned __int128)(0xFFFFFFFFFFFFFFFFULL) should be 0x0000...FFFF.
                if matches!(target_ir_ty, IrType::I128 | IrType::U128) {
                    let src_ty = self.get_expr_type(inner);
                    let v128 = if src_ty.is_unsigned() {
                        // Zero-extend: u64 -> i128
                        bits as i128
                    } else {
                        // Sign-extend: u64 -> i64 (reinterpret) -> i128 (sign-extend)
                        (bits as i64) as i128
                    };
                    return Some(IrConst::I128(v128));
                }

                // Truncate to target width
                let truncated = if target_width >= 64 {
                    bits
                } else {
                    bits & ((1u64 << target_width) - 1)
                };

                // Convert to IrConst based on target type
                let result = match target_ir_ty {
                    IrType::I8 => IrConst::I8(truncated as i8),
                    IrType::U8 => IrConst::I64(truncated as u8 as i64),
                    IrType::I16 => IrConst::I16(truncated as i16),
                    IrType::U16 => IrConst::I64(truncated as u16 as i64),
                    IrType::I32 => IrConst::I32(truncated as i32),
                    IrType::U32 => IrConst::I64(truncated as u32 as i64),
                    IrType::I64 | IrType::U64 | IrType::Ptr => IrConst::I64(truncated as i64),
                    IrType::I128 | IrType::U128 => unreachable!("handled above"),
                    IrType::F32 => {
                        let int_val = if target_signed { truncated as i64 as f32 } else { truncated as u64 as f32 };
                        IrConst::F32(int_val)
                    }
                    IrType::F64 => {
                        let int_val = if target_signed { truncated as i64 as f64 } else { truncated as u64 as f64 };
                        IrConst::F64(int_val)
                    }
                    _ => return None,
                };
                Some(result)
            }
            Expr::Identifier(name, _) => {
                // If a local variable exists with this name, it shadows any enum constant.
                // A non-const local is not a compile-time constant, so return None.
                // A const local's value is checked below.
                let is_local = self.func_state.as_ref()
                    .map_or(false, |fs| fs.locals.contains_key(name));

                if !is_local {
                    // Look up enum constants (only when not shadowed by a local variable)
                    if let Some(&val) = self.types.enum_constants.get(name) {
                        return Some(IrConst::I64(val));
                    }
                }
                // Look up const-qualified local variable values
                // (e.g., const int len = 5000; int arr[len];)
                if let Some(ref fs) = self.func_state {
                    if let Some(&val) = fs.const_local_values.get(name) {
                        return Some(IrConst::I64(val));
                    }
                }
                None
            }
            Expr::Sizeof(arg, _) => {
                let size = match arg.as_ref() {
                    SizeofArg::Type(ts) => self.sizeof_type(ts),
                    SizeofArg::Expr(e) => self.sizeof_expr(e),
                };
                Some(IrConst::I64(size as i64))
            }
            Expr::Alignof(ref ts, _) => {
                let align = self.alignof_type(ts);
                Some(IrConst::I64(align as i64))
            }
            Expr::AlignofExpr(ref inner_expr, _) => {
                let align = self.alignof_expr(inner_expr);
                Some(IrConst::I64(align as i64))
            }
            Expr::Conditional(cond, then_e, else_e, _) => {
                // Ternary in constant expr: evaluate condition and pick branch
                let cond_val = self.eval_const_expr(cond)?;
                if cond_val.is_nonzero() {
                    self.eval_const_expr(then_e)
                } else {
                    self.eval_const_expr(else_e)
                }
            }
            Expr::GnuConditional(cond, else_e, _) => {
                let cond_val = self.eval_const_expr(cond)?;
                if cond_val.is_nonzero() {
                    Some(cond_val) // condition value is used as result
                } else {
                    self.eval_const_expr(else_e)
                }
            }
            Expr::UnaryOp(UnaryOp::LogicalNot, inner, _) => {
                let val = self.eval_const_expr(inner)?;
                Some(IrConst::I64(if val.is_nonzero() { 0 } else { 1 }))
            }
            // Handle &((type*)0)->member pattern (offsetof)
            Expr::AddressOf(inner, _) => {
                self.eval_offsetof_pattern(inner)
            }
            Expr::BuiltinTypesCompatibleP(ref type1, ref type2, _) => {
                let result = self.eval_types_compatible(type1, type2);
                Some(IrConst::I64(result as i64))
            }
            // Handle compile-time builtin function calls in constant expressions.
            // __builtin_choose_expr(const_expr, expr1, expr2) selects expr1 or expr2
            // at compile time. __builtin_constant_p(expr) returns 1 if expr is a
            // compile-time constant, 0 otherwise. These are needed for global
            // initializer contexts where the result must be a constant.
            Expr::FunctionCall(func, args, _) => {
                if let Expr::Identifier(name, _) = func.as_ref() {
                    match name.as_str() {
                        "__builtin_choose_expr" if args.len() >= 3 => {
                            let cond = self.eval_const_expr(&args[0])?;
                            if cond.is_nonzero() {
                                self.eval_const_expr(&args[1])
                            } else {
                                self.eval_const_expr(&args[2])
                            }
                        }
                        "__builtin_constant_p" => {
                            let is_const = if let Some(arg) = args.first() {
                                self.eval_const_expr(arg).is_some()
                            } else {
                                false
                            };
                            Some(IrConst::I32(if is_const { 1 } else { 0 }))
                        }
                        "__builtin_expect" | "__builtin_expect_with_probability" => {
                            // __builtin_expect(val, expected) -> val
                            if let Some(arg) = args.first() {
                                self.eval_const_expr(arg)
                            } else {
                                None
                            }
                        }
                        "__builtin_bswap16" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u16;
                            Some(IrConst::I32(v.swap_bytes() as i32))
                        }
                        "__builtin_bswap32" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u32;
                            Some(IrConst::I32(v.swap_bytes() as i32))
                        }
                        "__builtin_bswap64" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u64;
                            Some(IrConst::I64(v.swap_bytes() as i64))
                        }
                        // __builtin_clz / __builtin_clzl / __builtin_clzll
                        "__builtin_clz" | "__builtin_clzl" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u32;
                            Some(IrConst::I32(v.leading_zeros() as i32))
                        }
                        "__builtin_clzll" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u64;
                            Some(IrConst::I32(v.leading_zeros() as i32))
                        }
                        // __builtin_ctz / __builtin_ctzl / __builtin_ctzll
                        "__builtin_ctz" | "__builtin_ctzl" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u32;
                            if v == 0 { Some(IrConst::I32(32)) }
                            else { Some(IrConst::I32(v.trailing_zeros() as i32)) }
                        }
                        "__builtin_ctzll" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u64;
                            if v == 0 { Some(IrConst::I32(64)) }
                            else { Some(IrConst::I32(v.trailing_zeros() as i32)) }
                        }
                        // __builtin_popcount / __builtin_popcountl / __builtin_popcountll
                        "__builtin_popcount" | "__builtin_popcountl" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u32;
                            Some(IrConst::I32(v.count_ones() as i32))
                        }
                        "__builtin_popcountll" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u64;
                            Some(IrConst::I32(v.count_ones() as i32))
                        }
                        // __builtin_ffs / __builtin_ffsl / __builtin_ffsll
                        "__builtin_ffs" | "__builtin_ffsl" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u32;
                            if v == 0 { Some(IrConst::I32(0)) }
                            else { Some(IrConst::I32(v.trailing_zeros() as i32 + 1)) }
                        }
                        "__builtin_ffsll" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u64;
                            if v == 0 { Some(IrConst::I32(0)) }
                            else { Some(IrConst::I32(v.trailing_zeros() as i32 + 1)) }
                        }
                        // __builtin_parity / __builtin_parityl / __builtin_parityll
                        "__builtin_parity" | "__builtin_parityl" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u32;
                            Some(IrConst::I32((v.count_ones() % 2) as i32))
                        }
                        "__builtin_parityll" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as u64;
                            Some(IrConst::I32((v.count_ones() % 2) as i32))
                        }
                        // __builtin_clrsb / __builtin_clrsbl / __builtin_clrsbll
                        "__builtin_clrsb" | "__builtin_clrsbl" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()? as i32;
                            let result = if v < 0 { (!v as u32).leading_zeros() as i32 - 1 }
                                         else { (v as u32).leading_zeros() as i32 - 1 };
                            Some(IrConst::I32(result))
                        }
                        "__builtin_clrsbll" => {
                            let val = self.eval_const_expr(args.first()?)?;
                            let v = val.to_i64()?;
                            let result = if v < 0 { (!v as u64).leading_zeros() as i32 - 1 }
                                         else { (v as u64).leading_zeros() as i32 - 1 };
                            Some(IrConst::I32(result))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            // Handle compound literals in constant expressions:
            // ((type) { value }) -> evaluate the inner initializer's scalar value.
            // This is critical for global/static array initializers where compound
            // literals like ((pgprot_t) { 0x120 }) must be evaluated at compile time.
            // Only treat as scalar if the type is NOT a multi-field aggregate;
            // multi-field structs must go through the proper struct init path.
            Expr::CompoundLiteral(ref type_spec, ref init, _) => {
                let cl_ctype = self.type_spec_to_ctype(type_spec);
                let is_multi_field_aggregate = match &cl_ctype {
                    CType::Struct(key) | CType::Union(key) => {
                        if let Some(layout) = self.types.struct_layouts.get(&**key) {
                            // A struct is an aggregate if it has multiple fields, OR if its
                            // single field is itself an aggregate (array/struct/union).
                            // Without this check, (arr_t){ {100, 200, 300} } where arr_t
                            // has one array field would be scalar-evaluated to just 100.
                            layout.fields.len() > 1
                                || layout.fields.iter().any(|f| {
                                    matches!(
                                        f.ty,
                                        CType::Array(..)
                                            | CType::Struct(..)
                                            | CType::Union(..)
                                    )
                                })
                        } else {
                            false
                        }
                    }
                    CType::Array(..) => true,
                    _ => false,
                };
                if is_multi_field_aggregate {
                    None
                } else {
                    self.eval_const_initializer_scalar(init)
                }
            }
            _ => None,
        }
    }

    /// Evaluate an initializer to a scalar constant for use in constant expressions.
    /// Handles both direct expressions and brace-wrapped lists (including nested ones
    /// like `{ { 42 } }` which occur when a struct compound literal has a single field).
    fn eval_const_initializer_scalar(&self, init: &Initializer) -> Option<IrConst> {
        match init {
            Initializer::Expr(expr) => self.eval_const_expr(expr),
            Initializer::List(items) => {
                // For a struct/union compound literal with one field,
                // the initializer list has one item whose value is the scalar.
                // Recurse into the first item.
                if let Some(first) = items.first() {
                    self.eval_const_initializer_scalar(&first.init)
                } else {
                    None
                }
            }
        }
    }

    /// Evaluate the offsetof pattern: &((type*)0)->member
    /// Also handles nested member access like &((type*)0)->data.x
    /// Returns Some(IrConst::I64(offset)) if the expression matches the pattern.
    fn eval_offsetof_pattern(&self, expr: &Expr) -> Option<IrConst> {
        let (offset, _ty) = self.eval_offsetof_pattern_with_type(expr)?;
        Some(IrConst::I64(offset as i64))
    }

    /// Evaluate an offsetof sub-expression, returning both the accumulated byte offset
    /// and the CType of the resulting expression (needed for chained member access).
    fn eval_offsetof_pattern_with_type(&self, expr: &Expr) -> Option<(usize, CType)> {
        match expr {
            Expr::PointerMemberAccess(base, field_name, _) => {
                // base should be (type*)0 - a cast of 0 to a pointer type
                let (type_spec, base_offset) = self.extract_null_pointer_cast_with_offset(base)?;
                let layout = self.get_struct_layout_for_type(&type_spec)?;
                let (field_offset, field_ty) = layout.field_offset(field_name, &self.types)?;
                Some((base_offset + field_offset, field_ty))
            }
            Expr::MemberAccess(base, field_name, _) => {
                // First try: base is *((type*)0) (deref pattern)
                if let Expr::Deref(inner, _) = base.as_ref() {
                    let (type_spec, base_offset) = self.extract_null_pointer_cast_with_offset(inner)?;
                    let layout = self.get_struct_layout_for_type(&type_spec)?;
                    let (field_offset, field_ty) = layout.field_offset(field_name, &self.types)?;
                    return Some((base_offset + field_offset, field_ty));
                }
                // Second try: base is itself an offsetof sub-expression (chained access)
                // e.g., ((type*)0)->data.x where base = ((type*)0)->data
                let (base_offset, base_type) = self.eval_offsetof_pattern_with_type(base)?;
                let struct_key = match &base_type {
                    CType::Struct(key) | CType::Union(key) => key.clone(),
                    _ => return None,
                };
                let layout = self.types.struct_layouts.get(&*struct_key)?;
                let (field_offset, field_ty) = layout.field_offset(field_name, &self.types)?;
                Some((base_offset + field_offset, field_ty))
            }
            Expr::ArraySubscript(base, index, _) => {
                // Handle &((type*)0)->member[index] pattern
                let (base_offset, base_type) = self.eval_offsetof_pattern_with_type(base)?;
                let idx_val = self.eval_const_expr(index)?;
                let idx = idx_val.to_i64()?;
                let elem_size = match &base_type {
                    CType::Array(elem, _) => self.resolve_ctype_size(elem),
                    _ => return None,
                };
                let elem_ty = match &base_type {
                    CType::Array(elem, _) => (**elem).clone(),
                    _ => return None,
                };
                Some(((base_offset as i64 + idx * elem_size as i64) as usize, elem_ty))
            }
            _ => None,
        }
    }

    /// Extract the struct type from a (type*)0 pattern, returning the base TypeSpecifier
    /// for the struct type and any accumulated offset from nested member access.
    fn extract_null_pointer_cast_with_offset(&self, expr: &Expr) -> Option<(TypeSpecifier, usize)> {
        match expr {
            Expr::Cast(ref type_spec, inner, _) => {
                // The type should be a Pointer to a struct
                if let TypeSpecifier::Pointer(inner_ts, _) = type_spec {
                    // Check that the inner expression is 0
                    if self.is_zero_expr(inner) {
                        return Some((*inner_ts.clone(), 0));
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Check if an expression evaluates to 0 (integer literal 0 or cast of 0).
    fn is_zero_expr(&self, expr: &Expr) -> bool {
        const_arith::is_zero_expr(expr)
    }

    /// Evaluate a constant expression, returning raw u64 bits and signedness.
    /// This preserves signedness information through cast chains.
    /// Signedness determines how the value is widened in the next cast.
    fn eval_const_expr_as_bits(&self, expr: &Expr) -> Option<(u64, bool)> {
        match expr {
            Expr::Cast(ref target_type, inner, _) => {
                let (bits, _src_signed) = self.eval_const_expr_as_bits(inner)?;
                let target_ir_ty = self.type_spec_to_ir(target_type);
                let target_width = target_ir_ty.size() * 8;
                let target_signed = matches!(target_ir_ty, IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64);
                Some(const_arith::truncate_and_extend_bits(bits, target_width, target_signed))
            }
            _ => {
                let val = self.eval_const_expr(expr)?;
                let bits = match &val {
                    IrConst::F32(v) => *v as i64 as u64,
                    IrConst::F64(v) => *v as i64 as u64,
                    _ => val.to_i64().unwrap_or(0) as u64,
                };
                Some((bits, true))
            }
        }
    }

    /// Cast a full 128-bit integer value to an IrType without going through u64 truncation.
    ///
    /// For targets <= 64 bits, extracts the lower bits. For 128-bit targets, preserves the
    /// full value. For float targets, uses value-based conversion from the full i128.
    fn cast_i128_to_ir_type(v128: i128, target: IrType, src_unsigned: bool) -> IrConst {
        let bits_lo = v128 as u64; // lower 64 bits
        match target {
            IrType::I8 => IrConst::I8(bits_lo as i8),
            IrType::U8 => IrConst::I64(bits_lo as u8 as i64),
            IrType::I16 => IrConst::I16(bits_lo as i16),
            IrType::U16 => IrConst::I64(bits_lo as u16 as i64),
            IrType::I32 => IrConst::I32(bits_lo as i32),
            IrType::U32 => IrConst::I64(bits_lo as u32 as i64),
            IrType::I64 | IrType::U64 | IrType::Ptr => IrConst::I64(bits_lo as i64),
            IrType::I128 | IrType::U128 => IrConst::I128(v128),
            IrType::F32 => {
                // int-to-float: signedness comes from the source type
                let fv = if src_unsigned { (v128 as u128) as f32 } else { v128 as f32 };
                IrConst::F32(fv)
            }
            IrType::F64 => {
                let fv = if src_unsigned { (v128 as u128) as f64 } else { v128 as f64 };
                IrConst::F64(fv)
            }
            IrType::F128 => {
                let fv = if src_unsigned { (v128 as u128) as f64 } else { v128 as f64 };
                IrConst::long_double(fv)
            }
            _ => IrConst::I128(v128), // fallback: preserve value
        }
    }

    /// Evaluate a constant binary operation.
    /// Uses both operand types for C's usual arithmetic conversions (C11 6.3.1.8),
    /// except for shifts where only the LHS type determines the result type (C11 6.5.7).
    /// Delegates arithmetic to the shared implementation in `common::const_arith`.
    fn eval_const_binop(&self, op: &BinOp, lhs: &IrConst, rhs: &IrConst, lhs_ty: IrType, rhs_ty: IrType) -> Option<IrConst> {
        let lhs_size = lhs_ty.size().max(4);
        let lhs_unsigned = lhs_ty.is_unsigned();
        let rhs_unsigned = rhs_ty.is_unsigned();
        let is_shift = matches!(op, BinOp::Shl | BinOp::Shr);

        // For shifts (C11 6.5.7): result type is the promoted LHS type only.
        // For other ops: apply usual arithmetic conversions using both operand types.
        let (is_32bit, is_unsigned) = if is_shift {
            (lhs_size <= 4, lhs_unsigned)
        } else {
            let rhs_size = rhs_ty.size().max(4);
            let result_size = lhs_size.max(rhs_size);
            let is_unsigned = if lhs_size == rhs_size {
                lhs_unsigned || rhs_unsigned
            } else if lhs_size > rhs_size {
                lhs_unsigned
            } else {
                rhs_unsigned
            };
            (result_size <= 4, is_unsigned)
        };
        const_arith::eval_const_binop(op, lhs, rhs, is_32bit, is_unsigned, lhs_unsigned, rhs_unsigned)
    }

    /// Promote a sub-int constant (I8/I16) to I32 for unary arithmetic,
    /// using unsigned zero-extension when the expression has an unsigned type.
    /// C11 6.3.1.1: unsigned char/short promote to int by zero-extending.
    fn promote_const_for_unary(&self, expr: &Expr, val: IrConst) -> IrConst {
        match &val {
            IrConst::I8(v) => {
                let is_unsigned = self.is_expr_unsigned_for_const(expr);
                if is_unsigned {
                    IrConst::I32(*v as u8 as i32)
                } else {
                    IrConst::I32(*v as i32)
                }
            }
            IrConst::I16(v) => {
                let is_unsigned = self.is_expr_unsigned_for_const(expr);
                if is_unsigned {
                    IrConst::I32(*v as u16 as i32)
                } else {
                    IrConst::I32(*v as i32)
                }
            }
            _ => val,
        }
    }

    /// Check if an expression has an unsigned type for constant evaluation.
    fn is_expr_unsigned_for_const(&self, expr: &Expr) -> bool {
        if let Expr::Cast(ref target_type, _, _) = expr {
            let ty = self.type_spec_to_ir(target_type);
            return ty.is_unsigned();
        }
        let ty = self.infer_expr_type(expr);
        ty.is_unsigned()
    }

    /// Try to constant-fold a binary operation from its parts.
    /// Used by lower_binary_op to avoid generating IR for constant expressions,
    /// ensuring correct C type semantics (especially 32-bit vs 64-bit width).
    pub(super) fn eval_const_expr_from_parts(&self, op: &BinOp, lhs: &Expr, rhs: &Expr) -> Option<IrConst> {
        let l = self.eval_const_expr(lhs)?;
        let r = self.eval_const_expr(rhs)?;
        let lhs_ty = self.infer_expr_type(lhs);
        let rhs_ty = self.infer_expr_type(rhs);
        let result = self.eval_const_binop(op, &l, &r, lhs_ty, rhs_ty)?;
        // Convert to I64 for IR operand compatibility (IR uses I64 for all int operations).
        Some(match result {
            IrConst::I32(v) => IrConst::I64(v as i64),
            IrConst::I8(v) => IrConst::I64(v as i64),
            IrConst::I16(v) => IrConst::I64(v as i64),
            other => other,
        })
    }

    /// Evaluate a constant expression and return as usize (for array index designators).
    pub(super) fn eval_const_expr_for_designator(&self, expr: &Expr) -> Option<usize> {
        self.eval_const_expr(expr).and_then(|v| v.to_usize())
    }

    /// Evaluate a compile-time pointer difference expression.
    /// Handles patterns like:
    ///   &arr[5] - &arr[0]              -> 5  (typed pointer subtraction)
    ///   (char*)&arr[5] - (char*)&arr[0] -> 20 (byte-level subtraction)
    ///   (long)&arr[5] - (long)&arr[0]  -> 20 (cast-to-integer subtraction)
    ///   (char*)&s.c - (char*)&s.a      -> 8  (struct member offset diff)
    ///
    /// Both operands must resolve to addresses within the same global symbol.
    /// The byte offsets are subtracted; for typed pointer subtraction the result
    /// is divided by the pointed-to element size.
    fn eval_const_ptr_diff(&self, lhs: &Expr, rhs: &Expr) -> Option<IrConst> {
        let lhs_addr = self.eval_global_addr_expr(lhs)?;
        let rhs_addr = self.eval_global_addr_expr(rhs)?;

        // Extract (symbol_name, byte_offset) from each side
        let (lhs_name, lhs_offset) = match &lhs_addr {
            GlobalInit::GlobalAddr(name) => (name.as_str(), 0i64),
            GlobalInit::GlobalAddrOffset(name, off) => (name.as_str(), *off),
            _ => return None,
        };
        let (rhs_name, rhs_offset) = match &rhs_addr {
            GlobalInit::GlobalAddr(name) => (name.as_str(), 0i64),
            GlobalInit::GlobalAddrOffset(name, off) => (name.as_str(), *off),
            _ => return None,
        };

        // Both must refer to the same global symbol
        if lhs_name != rhs_name {
            return None;
        }

        let byte_diff = lhs_offset - rhs_offset;

        // Determine if this is typed pointer subtraction (result in elements)
        // or byte/integer subtraction (result in bytes).
        // For typed pointer subtraction (ptr - ptr where both are non-void,
        // non-char pointers), divide by the element size.
        let result = if self.expr_is_pointer(lhs) && self.expr_is_pointer(rhs) {
            let elem_size = self.get_pointer_elem_size_from_expr(lhs) as i64;
            if elem_size > 1 {
                byte_diff / elem_size
            } else {
                byte_diff
            }
        } else {
            // Cast-to-integer subtraction: (long)&x - (long)&y, or
            // char*/void* subtraction: result is in bytes
            byte_diff
        };

        Some(IrConst::I64(result))
    }

    /// Convert an IrConst to i64. Delegates to IrConst::to_i64().
    pub(super) fn const_to_i64(&self, c: &IrConst) -> Option<i64> {
        c.to_i64()
    }

    /// Coerce a constant to the target type, using the source expression's type for signedness.
    pub(super) fn coerce_const_to_type_with_src(&self, val: IrConst, target_ty: IrType, src_ty: IrType) -> IrConst {
        val.coerce_to_with_src(target_ty, Some(src_ty))
    }

    /// Collect array dimensions from nested Array type specifiers.
    /// Extract an integer value from any integer literal expression (Int, UInt, Long, ULong).
    /// Used for array sizes and other compile-time integer expressions.
    pub(super) fn expr_as_array_size(&self, expr: &Expr) -> Option<i64> {
        // Try simple literals first (fast path)
        match expr {
            Expr::IntLiteral(n, _) | Expr::LongLiteral(n, _) => return Some(*n),
            Expr::UIntLiteral(n, _) | Expr::ULongLiteral(n, _) => return Some(*n as i64),
            _ => {}
        }
        // Fall back to full constant expression evaluation (handles sizeof, arithmetic, etc.)
        if let Some(val) = self.eval_const_expr(expr) {
            return self.const_to_i64(&val);
        }
        None
    }
}

