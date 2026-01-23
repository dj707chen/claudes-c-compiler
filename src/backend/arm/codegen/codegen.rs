use std::collections::{HashMap, HashSet};
use crate::ir::ir::*;
use crate::common::types::IrType;
use crate::backend::common::{self, AsmOutput, PtrDirective};

/// AArch64 code generator. Produces assembly text from IR.
pub struct ArmCodegen {
    out: AsmOutput,
    stack_offset: i64,
    value_locations: HashMap<u32, i64>, // value -> stack offset from sp
    /// Track which Values are allocas (their stack slot IS the data, not a pointer to data).
    alloca_values: HashSet<u32>,
}

impl ArmCodegen {
    pub fn new() -> Self {
        Self {
            out: AsmOutput::new(),
            stack_offset: 0,
            value_locations: HashMap::new(),
            alloca_values: HashSet::new(),
        }
    }

    pub fn generate(mut self, module: &IrModule) -> String {
        // Emit data sections using shared code
        common::emit_data_sections(&mut self.out, module, PtrDirective::Xword);

        // Text section
        self.out.emit(".section .text");
        for func in &module.functions {
            if !func.is_declaration {
                self.generate_function(func);
            }
        }

        self.out.buf
    }

    fn emit(&mut self, s: &str) {
        self.out.emit(s);
    }

    fn generate_function(&mut self, func: &IrFunction) {
        self.stack_offset = 0;
        self.value_locations.clear();
        self.alloca_values.clear();

        self.emit(&format!(".globl {}", func.name));
        self.emit(&format!(".type {}, %function", func.name));
        self.emit(&format!("{}:", func.name));

        // Calculate stack space
        let stack_space = self.calculate_stack_space(func);
        let aligned_space = ((stack_space + 15) & !15) + 16; // +16 for fp/lr

        // Prologue: save fp, lr and allocate stack
        self.emit(&format!("    stp x29, x30, [sp, #-{}]!", aligned_space));
        self.emit("    mov x29, sp");

        // Store parameters (x0-x7 for first 8 args)
        let arg_regs = ["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7"];
        for (i, param) in func.params.iter().enumerate() {
            if i < 8 && !param.name.is_empty() {
                self.store_param(func, i, &arg_regs);
            }
        }

        // Generate code for each basic block
        for block in &func.blocks {
            if block.label != "entry" {
                self.emit(&format!("{}:", block.label));
            }
            for inst in &block.instructions {
                self.generate_instruction(inst);
            }
            self.generate_terminator(&block.terminator, aligned_space);
        }

        self.emit(&format!(".size {}, .-{}", func.name, func.name));
        self.emit("");
    }

    fn calculate_stack_space(&mut self, func: &IrFunction) -> i64 {
        let mut space: i64 = 16; // start after fp/lr save area
        for block in &func.blocks {
            for inst in &block.instructions {
                if let Some(dest) = common::instruction_dest(inst) {
                    let size = if let Instruction::Alloca { size, dest: alloca_dest, .. } = inst {
                        self.alloca_values.insert(alloca_dest.0);
                        ((*size as i64 + 7) & !7).max(8)
                    } else {
                        8
                    };
                    self.value_locations.insert(dest.0, space);
                    space += size;
                }
            }
        }
        space
    }

    fn store_param(&mut self, func: &IrFunction, arg_idx: usize, arg_regs: &[&str]) {
        if let Some(block) = func.blocks.first() {
            let alloca_idx = block.instructions.iter()
                .filter(|i| matches!(i, Instruction::Alloca { .. }))
                .nth(arg_idx);
            if let Some(Instruction::Alloca { dest, ty, .. }) = alloca_idx {
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    let store_instr = Self::str_for_type(*ty);
                    let reg = Self::reg_for_type(arg_regs[arg_idx], *ty);
                    self.emit(&format!("    {} {}, [sp, #{}]", store_instr, reg, offset));
                }
            }
        }
    }

    fn generate_instruction(&mut self, inst: &Instruction) {
        match inst {
            Instruction::Alloca { .. } => {}
            Instruction::Store { val, ptr, ty } => {
                self.operand_to_x0(val);
                if let Some(&offset) = self.value_locations.get(&ptr.0) {
                    if self.alloca_values.contains(&ptr.0) {
                        let store_instr = Self::str_for_type(*ty);
                        let reg = Self::reg_for_type("x0", *ty);
                        self.emit(&format!("    {} {}, [sp, #{}]", store_instr, reg, offset));
                    } else {
                        self.emit("    mov x1, x0");
                        self.emit(&format!("    ldr x2, [sp, #{}]", offset));
                        let store_instr = Self::str_for_type(*ty);
                        let reg = Self::reg_for_type("x1", *ty);
                        self.emit(&format!("    {} {}, [x2]", store_instr, reg));
                    }
                }
            }
            Instruction::Load { dest, ptr, ty } => {
                if let Some(&ptr_off) = self.value_locations.get(&ptr.0) {
                    let load_instr = Self::ldr_for_type(*ty);
                    let dest_reg = Self::load_dest_reg(*ty);
                    if self.alloca_values.contains(&ptr.0) {
                        self.emit(&format!("    {} {}, [sp, #{}]", load_instr, dest_reg, ptr_off));
                    } else {
                        self.emit(&format!("    ldr x0, [sp, #{}]", ptr_off));
                        self.emit("    mov x1, x0");
                        self.emit(&format!("    {} {}, [x1]", load_instr, dest_reg));
                    }
                    if let Some(&dest_off) = self.value_locations.get(&dest.0) {
                        self.emit(&format!("    str x0, [sp, #{}]", dest_off));
                    }
                }
            }
            Instruction::BinOp { dest, op, lhs, rhs, ty } => {
                self.operand_to_x0(lhs);
                self.emit("    mov x1, x0");
                self.operand_to_x0(rhs);
                self.emit("    mov x2, x0");

                // Use 32-bit (w-register) operations for I32/U32 types
                let use_32bit = *ty == IrType::I32 || *ty == IrType::U32;
                let is_unsigned = ty.is_unsigned();

                if use_32bit {
                    match op {
                        IrBinOp::Add => {
                            self.emit("    add w0, w1, w2");
                            if !is_unsigned { self.emit("    sxtw x0, w0"); }
                        }
                        IrBinOp::Sub => {
                            self.emit("    sub w0, w1, w2");
                            if !is_unsigned { self.emit("    sxtw x0, w0"); }
                        }
                        IrBinOp::Mul => {
                            self.emit("    mul w0, w1, w2");
                            if !is_unsigned { self.emit("    sxtw x0, w0"); }
                        }
                        IrBinOp::SDiv => {
                            self.emit("    sdiv w0, w1, w2");
                            self.emit("    sxtw x0, w0");
                        }
                        IrBinOp::UDiv => {
                            self.emit("    udiv w0, w1, w2");
                            // w-register result is already zero-extended in x0
                        }
                        IrBinOp::SRem => {
                            self.emit("    sdiv w3, w1, w2");
                            self.emit("    msub w0, w3, w2, w1");
                            self.emit("    sxtw x0, w0");
                        }
                        IrBinOp::URem => {
                            self.emit("    udiv w3, w1, w2");
                            self.emit("    msub w0, w3, w2, w1");
                            // w-register result is already zero-extended in x0
                        }
                        IrBinOp::And => self.emit("    and w0, w1, w2"),
                        IrBinOp::Or => self.emit("    orr w0, w1, w2"),
                        IrBinOp::Xor => self.emit("    eor w0, w1, w2"),
                        IrBinOp::Shl => self.emit("    lsl w0, w1, w2"),
                        IrBinOp::AShr => self.emit("    asr w0, w1, w2"),
                        IrBinOp::LShr => self.emit("    lsr w0, w1, w2"),
                    }
                } else {
                    match op {
                        IrBinOp::Add => self.emit("    add x0, x1, x2"),
                        IrBinOp::Sub => self.emit("    sub x0, x1, x2"),
                        IrBinOp::Mul => self.emit("    mul x0, x1, x2"),
                        IrBinOp::SDiv => self.emit("    sdiv x0, x1, x2"),
                        IrBinOp::UDiv => self.emit("    udiv x0, x1, x2"),
                        IrBinOp::SRem => {
                            self.emit("    sdiv x3, x1, x2");
                            self.emit("    msub x0, x3, x2, x1");
                        }
                        IrBinOp::URem => {
                            self.emit("    udiv x3, x1, x2");
                            self.emit("    msub x0, x3, x2, x1");
                        }
                        IrBinOp::And => self.emit("    and x0, x1, x2"),
                        IrBinOp::Or => self.emit("    orr x0, x1, x2"),
                        IrBinOp::Xor => self.emit("    eor x0, x1, x2"),
                        IrBinOp::Shl => self.emit("    lsl x0, x1, x2"),
                        IrBinOp::AShr => self.emit("    asr x0, x1, x2"),
                        IrBinOp::LShr => self.emit("    lsr x0, x1, x2"),
                    }
                }

                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", offset));
                }
            }
            Instruction::UnaryOp { dest, op, src, .. } => {
                self.operand_to_x0(src);
                match op {
                    IrUnaryOp::Neg => self.emit("    neg x0, x0"),
                    IrUnaryOp::Not => self.emit("    mvn x0, x0"),
                }
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", offset));
                }
            }
            Instruction::Cmp { dest, op, lhs, rhs, ty } => {
                self.operand_to_x0(lhs);
                self.emit("    mov x1, x0");
                self.operand_to_x0(rhs);
                // Use 32-bit compare for I32/U32 to match C semantics
                let use_32bit = *ty == IrType::I32 || *ty == IrType::U32
                    || *ty == IrType::I8 || *ty == IrType::U8
                    || *ty == IrType::I16 || *ty == IrType::U16;
                if use_32bit {
                    self.emit("    cmp w1, w0");
                } else {
                    self.emit("    cmp x1, x0");
                }

                let cond = match op {
                    IrCmpOp::Eq => "eq",
                    IrCmpOp::Ne => "ne",
                    IrCmpOp::Slt => "lt",
                    IrCmpOp::Sle => "le",
                    IrCmpOp::Sgt => "gt",
                    IrCmpOp::Sge => "ge",
                    IrCmpOp::Ult => "lo",
                    IrCmpOp::Ule => "ls",
                    IrCmpOp::Ugt => "hi",
                    IrCmpOp::Uge => "hs",
                };
                self.emit(&format!("    cset x0, {}", cond));

                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", offset));
                }
            }
            Instruction::Call { dest, func, args, .. } => {
                self.emit_call(args, Some(func), None, *dest);
            }
            Instruction::CallIndirect { dest, func_ptr, args, .. } => {
                self.emit_call(args, None, Some(func_ptr), *dest);
            }
            Instruction::GlobalAddr { dest, name } => {
                self.emit(&format!("    adrp x0, {}", name));
                self.emit(&format!("    add x0, x0, :lo12:{}", name));
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", offset));
                }
            }
            Instruction::Copy { dest, src } => {
                self.operand_to_x0(src);
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", offset));
                }
            }
            Instruction::Cast { dest, src, from_ty, to_ty } => {
                self.operand_to_x0(src);
                self.emit_cast(*from_ty, *to_ty);
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", offset));
                }
            }
            Instruction::GetElementPtr { dest, base, offset, .. } => {
                if let Some(&base_off) = self.value_locations.get(&base.0) {
                    if self.alloca_values.contains(&base.0) {
                        self.emit(&format!("    add x1, sp, #{}", base_off));
                    } else {
                        self.emit(&format!("    ldr x1, [sp, #{}]", base_off));
                    }
                }
                self.operand_to_x0(offset);
                self.emit("    add x0, x1, x0");
                if let Some(&dest_off) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    str x0, [sp, #{}]", dest_off));
                }
            }
        }
    }

    /// Emit a function call (direct or indirect). Handles AArch64 calling convention.
    fn emit_call(&mut self, args: &[Operand], direct_name: Option<&str>, func_ptr: Option<&Operand>, dest: Option<Value>) {
        let arg_regs = ["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7"];
        let tmp_regs = ["x9", "x10", "x11", "x12", "x13", "x14", "x15", "x16"];
        let num_args = args.len().min(8);

        // Load all args into temp registers first (to avoid clobbering)
        for (i, arg) in args.iter().enumerate().take(num_args) {
            self.operand_to_x0(arg);
            self.emit(&format!("    mov {}, x0", tmp_regs[i]));
        }
        // Move from temps to arg registers
        for i in 0..num_args {
            self.emit(&format!("    mov {}, {}", arg_regs[i], tmp_regs[i]));
        }

        if let Some(name) = direct_name {
            self.emit(&format!("    bl {}", name));
        } else if let Some(ptr) = func_ptr {
            self.operand_to_x0(ptr);
            self.emit("    mov x17, x0");
            self.emit("    blr x17");
        }

        if let Some(dest) = dest {
            if let Some(&offset) = self.value_locations.get(&dest.0) {
                self.emit(&format!("    str x0, [sp, #{}]", offset));
            }
        }
    }

    fn generate_terminator(&mut self, term: &Terminator, frame_size: i64) {
        match term {
            Terminator::Return(val) => {
                if let Some(val) = val {
                    self.operand_to_x0(val);
                }
                self.emit(&format!("    ldp x29, x30, [sp], #{}", frame_size));
                self.emit("    ret");
            }
            Terminator::Branch(label) => {
                self.emit(&format!("    b {}", label));
            }
            Terminator::CondBranch { cond, true_label, false_label } => {
                self.operand_to_x0(cond);
                self.emit(&format!("    cbnz x0, {}", true_label));
                self.emit(&format!("    b {}", false_label));
            }
            Terminator::Unreachable => {
                self.emit("    brk #0");
            }
        }
    }

    fn str_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "strb",
            IrType::I16 | IrType::U16 => "strh",
            _ => "str",
        }
    }

    fn ldr_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 => "ldrsb",
            IrType::U8 => "ldrb",
            IrType::I16 => "ldrsh",
            IrType::U16 => "ldrh",
            IrType::I32 => "ldrsw",
            IrType::U32 => "ldr",
            _ => "ldr",
        }
    }

    fn load_dest_reg(ty: IrType) -> &'static str {
        match ty {
            IrType::U8 | IrType::U16 | IrType::U32 => "w0",
            _ => "x0",
        }
    }

    /// Get the appropriate register name for a given base and type.
    /// On AArch64, wN is the 32-bit alias of xN.
    /// For byte/halfword/word stores, strb/strh/str use w-registers.
    fn reg_for_type(base: &str, ty: IrType) -> &'static str {
        let use_w = matches!(ty,
            IrType::I8 | IrType::U8 | IrType::I16 | IrType::U16 |
            IrType::I32 | IrType::U32
        );
        match base {
            "x0" => if use_w { "w0" } else { "x0" },
            "x1" => if use_w { "w1" } else { "x1" },
            "x2" => if use_w { "w2" } else { "x2" },
            "x3" => if use_w { "w3" } else { "x3" },
            "x4" => if use_w { "w4" } else { "x4" },
            "x5" => if use_w { "w5" } else { "x5" },
            "x6" => if use_w { "w6" } else { "x6" },
            "x7" => if use_w { "w7" } else { "x7" },
            _ => "x0", // fallback
        }
    }

    /// Emit a type cast instruction sequence for AArch64.
    fn emit_cast(&mut self, from_ty: IrType, to_ty: IrType) {
        // Same type: no cast needed
        if from_ty == to_ty {
            return;
        }
        // Same size integers: just reinterpret signedness
        if from_ty.size() == to_ty.size() && from_ty.is_integer() && to_ty.is_integer() {
            return;
        }

        let from_size = from_ty.size();
        let to_size = to_ty.size();

        if to_size > from_size {
            // Widening cast
            if from_ty.is_unsigned() {
                // Zero-extend for unsigned types
                match from_ty {
                    IrType::U8 => { self.emit("    and x0, x0, #0xff"); }
                    IrType::U16 => { self.emit("    and x0, x0, #0xffff"); }
                    IrType::U32 => { self.emit("    mov w0, w0"); } // implicit zero-extend
                    _ => {}
                }
            } else {
                // Sign-extend for signed types
                match from_ty {
                    IrType::I8 => { self.emit("    sxtb x0, w0"); }
                    IrType::I16 => { self.emit("    sxth x0, w0"); }
                    IrType::I32 => { self.emit("    sxtw x0, w0"); }
                    _ => {}
                }
            }
        } else if to_size < from_size {
            // Narrowing cast: mask to target width
            match to_ty {
                IrType::I8 | IrType::U8 => { self.emit("    and x0, x0, #0xff"); }
                IrType::I16 | IrType::U16 => { self.emit("    and x0, x0, #0xffff"); }
                IrType::I32 | IrType::U32 => { self.emit("    mov w0, w0"); } // truncate to 32-bit
                _ => {}
            }
        } else {
            // Same size: pointer <-> integer conversions (no-op on AArch64)
        }
    }

    fn operand_to_x0(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) => self.emit(&format!("    mov x0, #{}", v)),
                    IrConst::I16(v) => self.emit(&format!("    mov x0, #{}", v)),
                    IrConst::I32(v) => {
                        if *v >= 0 && *v <= 65535 {
                            self.emit(&format!("    mov x0, #{}", v));
                        } else if *v < 0 && *v >= -65536 {
                            self.emit(&format!("    mov x0, #{}", v));
                        } else {
                            self.emit(&format!("    mov x0, #{}", *v as u32 & 0xffff));
                            self.emit(&format!("    movk x0, #{}, lsl #16", (*v as u32 >> 16) & 0xffff));
                        }
                    }
                    IrConst::I64(v) => {
                        if *v >= 0 && *v <= 65535 {
                            self.emit(&format!("    mov x0, #{}", v));
                        } else if *v < 0 && *v >= -65536 {
                            self.emit(&format!("    mov x0, #{}", v));
                        } else {
                            self.emit(&format!("    mov x0, #{}", *v as u64 & 0xffff));
                            if (*v as u64 >> 16) & 0xffff != 0 {
                                self.emit(&format!("    movk x0, #{}, lsl #16", (*v as u64 >> 16) & 0xffff));
                            }
                            if (*v as u64 >> 32) & 0xffff != 0 {
                                self.emit(&format!("    movk x0, #{}, lsl #32", (*v as u64 >> 32) & 0xffff));
                            }
                            if (*v as u64 >> 48) & 0xffff != 0 {
                                self.emit(&format!("    movk x0, #{}, lsl #48", (*v as u64 >> 48) & 0xffff));
                            }
                        }
                    }
                    IrConst::F32(_) | IrConst::F64(_) => {
                        self.emit("    mov x0, #0");
                    }
                    IrConst::Zero => self.emit("    mov x0, #0"),
                }
            }
            Operand::Value(v) => {
                if let Some(&offset) = self.value_locations.get(&v.0) {
                    if self.alloca_values.contains(&v.0) {
                        self.emit(&format!("    add x0, sp, #{}", offset));
                    } else {
                        self.emit(&format!("    ldr x0, [sp, #{}]", offset));
                    }
                } else {
                    self.emit("    mov x0, #0");
                }
            }
        }
    }
}

impl Default for ArmCodegen {
    fn default() -> Self {
        Self::new()
    }
}
