use crate::frontend::parser::ast::*;
use crate::ir::ir::*;
use crate::common::types::IrType;
use super::lowering::{Lowerer, LValue};

impl Lowerer {
    /// Try to get the lvalue (address) of an expression.
    /// Returns Some(LValue) if the expression is an lvalue, None otherwise.
    pub(super) fn lower_lvalue(&mut self, expr: &Expr) -> Option<LValue> {
        match expr {
            Expr::Identifier(name, _) => {
                if let Some(info) = self.locals.get(name).cloned() {
                    Some(LValue::Variable(info.alloca))
                } else if self.globals.contains_key(name) {
                    // Global variable: emit GlobalAddr to get its address
                    let addr = self.fresh_value();
                    self.emit(Instruction::GlobalAddr { dest: addr, name: name.clone() });
                    Some(LValue::Address(addr))
                } else {
                    None
                }
            }
            Expr::Deref(inner, _) => {
                // *ptr -> the address is the value of ptr
                let ptr_val = self.lower_expr(inner);
                match ptr_val {
                    Operand::Value(v) => Some(LValue::Address(v)),
                    Operand::Const(_) => {
                        // Constant address - copy to a value
                        let dest = self.fresh_value();
                        self.emit(Instruction::Copy { dest, src: ptr_val });
                        Some(LValue::Address(dest))
                    }
                }
            }
            Expr::ArraySubscript(base, index, _) => {
                // base[index] -> compute address of element
                let addr = self.compute_array_element_addr(base, index);
                Some(LValue::Address(addr))
            }
            Expr::MemberAccess(base_expr, field_name, _) => {
                // s.field as lvalue -> compute address of the field
                let (field_offset, _field_ty) = self.resolve_member_access(base_expr, field_name);
                let base_addr = self.get_struct_base_addr(base_expr);
                let field_addr = self.fresh_value();
                self.emit(Instruction::GetElementPtr {
                    dest: field_addr,
                    base: base_addr,
                    offset: Operand::Const(IrConst::I64(field_offset as i64)),
                    ty: IrType::Ptr,
                });
                Some(LValue::Address(field_addr))
            }
            Expr::PointerMemberAccess(base_expr, field_name, _) => {
                // p->field as lvalue -> load pointer, compute field address
                let ptr_val = self.lower_expr(base_expr);
                let base_addr = match ptr_val {
                    Operand::Value(v) => v,
                    Operand::Const(_) => {
                        let tmp = self.fresh_value();
                        self.emit(Instruction::Copy { dest: tmp, src: ptr_val });
                        tmp
                    }
                };
                let (field_offset, _field_ty) = self.resolve_pointer_member_access(base_expr, field_name);
                let field_addr = self.fresh_value();
                self.emit(Instruction::GetElementPtr {
                    dest: field_addr,
                    base: base_addr,
                    offset: Operand::Const(IrConst::I64(field_offset as i64)),
                    ty: IrType::Ptr,
                });
                Some(LValue::Address(field_addr))
            }
            _ => None,
        }
    }

    /// Get the address (as a Value) from an LValue.
    pub(super) fn lvalue_addr(&self, lv: &LValue) -> Value {
        match lv {
            LValue::Variable(v) => *v,
            LValue::Address(v) => *v,
        }
    }

    /// Load the value from an lvalue with a specific type.
    pub(super) fn load_lvalue_typed(&mut self, lv: &LValue, ty: IrType) -> Operand {
        let addr = self.lvalue_addr(lv);
        let dest = self.fresh_value();
        self.emit(Instruction::Load { dest, ptr: addr, ty });
        Operand::Value(dest)
    }

    /// Load the value from an lvalue (defaults to I64 for backwards compat).
    #[allow(dead_code)]
    pub(super) fn load_lvalue(&mut self, lv: &LValue) -> Operand {
        self.load_lvalue_typed(lv, IrType::I64)
    }

    /// Store a value to an lvalue with a specific type.
    pub(super) fn store_lvalue_typed(&mut self, lv: &LValue, val: Operand, ty: IrType) {
        let addr = self.lvalue_addr(lv);
        self.emit(Instruction::Store { val, ptr: addr, ty });
    }

    /// Store a value to an lvalue (defaults to I64 for backwards compat).
    #[allow(dead_code)]
    pub(super) fn store_lvalue(&mut self, lv: &LValue, val: Operand) {
        self.store_lvalue_typed(lv, val, IrType::I64);
    }

    /// Compute the address of an array element: base_addr + index * elem_size.
    pub(super) fn compute_array_element_addr(&mut self, base: &Expr, index: &Expr) -> Value {
        let index_val = self.lower_expr(index);

        // Determine the element size. For arrays declared with a known type,
        // we use elem_size from LocalInfo. Default to 8 (int/pointer size on x86-64)
        // but try to use 4 for int arrays (most common case).
        let elem_size = self.get_array_elem_size(base);

        // Get the base address
        let base_addr = self.get_array_base_addr(base);

        // Compute offset = index * elem_size
        let offset = if elem_size == 1 {
            // No multiplication needed
            index_val
        } else {
            let size_const = Operand::Const(IrConst::I64(elem_size as i64));
            let mul_dest = self.fresh_value();
            self.emit(Instruction::BinOp {
                dest: mul_dest,
                op: IrBinOp::Mul,
                lhs: index_val,
                rhs: size_const,
                ty: IrType::I64,
            });
            Operand::Value(mul_dest)
        };

        // GEP: base + offset
        let addr = self.fresh_value();
        let base_val = match base_addr {
            Operand::Value(v) => v,
            Operand::Const(_) => {
                let tmp = self.fresh_value();
                self.emit(Instruction::Copy { dest: tmp, src: base_addr });
                tmp
            }
        };
        self.emit(Instruction::GetElementPtr {
            dest: addr,
            base: base_val,
            offset,
            ty: IrType::I64,
        });
        addr
    }

    /// Get the base address for an array expression.
    /// For declared arrays, this is the alloca itself (the alloca IS the array).
    /// For pointers, this is the loaded pointer value.
    pub(super) fn get_array_base_addr(&mut self, base: &Expr) -> Operand {
        match base {
            Expr::Identifier(name, _) => {
                if let Some(info) = self.locals.get(name).cloned() {
                    if info.is_array {
                        // The alloca IS the base address of the array
                        return Operand::Value(info.alloca);
                    } else {
                        // It's a pointer - load it (pointers are always 8 bytes/Ptr type)
                        let loaded = self.fresh_value();
                        self.emit(Instruction::Load { dest: loaded, ptr: info.alloca, ty: IrType::Ptr });
                        return Operand::Value(loaded);
                    }
                }
                if let Some(ginfo) = self.globals.get(name).cloned() {
                    // Global variable
                    let addr = self.fresh_value();
                    self.emit(Instruction::GlobalAddr { dest: addr, name: name.clone() });
                    if ginfo.is_array {
                        // Global array: address IS the base pointer
                        return Operand::Value(addr);
                    } else {
                        // Global pointer: load the pointer value
                        let loaded = self.fresh_value();
                        self.emit(Instruction::Load { dest: loaded, ptr: addr, ty: IrType::Ptr });
                        return Operand::Value(loaded);
                    }
                }
                // Fall through to generic lowering
                self.lower_expr(base)
            }
            _ => {
                // Generic case: evaluate the expression (it should be a pointer)
                self.lower_expr(base)
            }
        }
    }

    /// Get the element size for array subscript. Tries to determine from context.
    pub(super) fn get_array_elem_size(&self, base: &Expr) -> usize {
        if let Expr::Identifier(name, _) = base {
            if let Some(info) = self.locals.get(name) {
                if info.elem_size > 0 {
                    return info.elem_size;
                }
            }
            if let Some(ginfo) = self.globals.get(name) {
                if ginfo.elem_size > 0 {
                    return ginfo.elem_size;
                }
            }
        }
        // Default element size: for pointers we don't know the element type,
        // use 8 bytes as a safe default for pointer dereferencing.
        // TODO: use type information from sema to determine correct size
        8
    }

    /// Compute the element size for a pointer type specifier.
    /// For `Pointer(Int)`, returns sizeof(int) = 4.
    /// For `Pointer(Char)`, returns 1.
    /// For `Pointer(Pointer(...))`, returns 8.
    pub(super) fn pointee_elem_size(&self, type_spec: &TypeSpecifier) -> usize {
        match type_spec {
            TypeSpecifier::Pointer(inner) => self.sizeof_type(inner),
            // Array parameters decay to pointers; elem_size is the element type size
            TypeSpecifier::Array(inner, _) => self.sizeof_type(inner),
            // Not a pointer type
            _ => 0,
        }
    }
}
