use std::collections::{HashMap, HashSet};
use crate::ir::ir::*;
use crate::common::types::IrType;
use crate::backend::common::{self, AsmOutput, PtrDirective};

/// RISC-V 64 code generator. Produces assembly text from IR.
pub struct RiscvCodegen {
    out: AsmOutput,
    stack_offset: i64,
    value_locations: HashMap<u32, i64>,
    /// Track which Values are allocas (their stack slot IS the data, not a pointer to data).
    alloca_values: HashSet<u32>,
}

impl RiscvCodegen {
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
        common::emit_data_sections(&mut self.out, module, PtrDirective::Dword);

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
        self.emit(&format!(".type {}, @function", func.name));
        self.emit(&format!("{}:", func.name));

        // Calculate stack space
        let stack_space = self.calculate_stack_space(func);
        let frame_size = ((stack_space + 15) & !15) + 16; // +16 for ra/s0

        // Prologue
        self.emit(&format!("    addi sp, sp, -{}", frame_size));
        self.emit(&format!("    sd ra, {}(sp)", frame_size - 8));
        self.emit(&format!("    sd s0, {}(sp)", frame_size - 16));
        self.emit(&format!("    addi s0, sp, {}", frame_size));

        // Store parameters (a0-a7)
        let arg_regs = ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"];
        for (i, param) in func.params.iter().enumerate() {
            if i < 8 && !param.name.is_empty() {
                self.store_param(func, i, &arg_regs);
            }
        }

        // Generate blocks
        for block in &func.blocks {
            if block.label != "entry" {
                self.emit(&format!("{}:", block.label));
            }
            for inst in &block.instructions {
                self.generate_instruction(inst);
            }
            self.generate_terminator(&block.terminator, frame_size);
        }

        self.emit(&format!(".size {}, .-{}", func.name, func.name));
        self.emit("");
    }

    fn calculate_stack_space(&mut self, func: &IrFunction) -> i64 {
        let mut space: i64 = 16;
        for block in &func.blocks {
            for inst in &block.instructions {
                if let Some(dest) = common::instruction_dest(inst) {
                    let size = if let Instruction::Alloca { size, dest: alloca_dest, .. } = inst {
                        self.alloca_values.insert(alloca_dest.0);
                        ((*size as i64 + 7) & !7).max(8)
                    } else {
                        8
                    };
                    space += size;
                    self.value_locations.insert(dest.0, -(space as i64));
                }
            }
        }
        space
    }

    fn store_param(&mut self, func: &IrFunction, arg_idx: usize, arg_regs: &[&str]) {
        if let Some(block) = func.blocks.first() {
            let alloca = block.instructions.iter()
                .filter(|i| matches!(i, Instruction::Alloca { .. }))
                .nth(arg_idx);
            if let Some(Instruction::Alloca { dest, ty, .. }) = alloca {
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    let store_instr = Self::store_for_type(*ty);
                    self.emit(&format!("    {} {}, {}(s0)", store_instr, arg_regs[arg_idx], offset));
                }
            }
        }
    }

    fn generate_instruction(&mut self, inst: &Instruction) {
        match inst {
            Instruction::Alloca { .. } => {}
            Instruction::Store { val, ptr, ty } => {
                self.operand_to_t0(val);
                if let Some(&offset) = self.value_locations.get(&ptr.0) {
                    if self.alloca_values.contains(&ptr.0) {
                        let store_instr = Self::store_for_type(*ty);
                        self.emit(&format!("    {} t0, {}(s0)", store_instr, offset));
                    } else {
                        self.emit("    mv t3, t0");
                        self.emit(&format!("    ld t4, {}(s0)", offset));
                        let store_instr = Self::store_for_type(*ty);
                        self.emit(&format!("    {} t3, 0(t4)", store_instr));
                    }
                }
            }
            Instruction::Load { dest, ptr, ty } => {
                if let Some(&ptr_off) = self.value_locations.get(&ptr.0) {
                    if self.alloca_values.contains(&ptr.0) {
                        let load_instr = Self::load_for_type(*ty);
                        self.emit(&format!("    {} t0, {}(s0)", load_instr, ptr_off));
                    } else {
                        self.emit(&format!("    ld t0, {}(s0)", ptr_off));
                        let load_instr = Self::load_for_type(*ty);
                        self.emit(&format!("    {} t0, 0(t0)", load_instr));
                    }
                    if let Some(&dest_off) = self.value_locations.get(&dest.0) {
                        self.emit(&format!("    sd t0, {}(s0)", dest_off));
                    }
                }
            }
            Instruction::BinOp { dest, op, lhs, rhs, ty } => {
                self.operand_to_t0(lhs);
                self.emit("    mv t1, t0");
                self.operand_to_t0(rhs);
                self.emit("    mv t2, t0");

                // Use 32-bit (W-suffix) operations for I32/U32 types
                // On RV64, *w instructions operate on lower 32 bits and sign-extend result
                let use_32bit = *ty == IrType::I32 || *ty == IrType::U32;

                if use_32bit {
                    match op {
                        IrBinOp::Add => self.emit("    addw t0, t1, t2"),
                        IrBinOp::Sub => self.emit("    subw t0, t1, t2"),
                        IrBinOp::Mul => self.emit("    mulw t0, t1, t2"),
                        IrBinOp::SDiv => self.emit("    divw t0, t1, t2"),
                        IrBinOp::UDiv => self.emit("    divuw t0, t1, t2"),
                        IrBinOp::SRem => self.emit("    remw t0, t1, t2"),
                        IrBinOp::URem => self.emit("    remuw t0, t1, t2"),
                        IrBinOp::And => self.emit("    and t0, t1, t2"),
                        IrBinOp::Or => self.emit("    or t0, t1, t2"),
                        IrBinOp::Xor => self.emit("    xor t0, t1, t2"),
                        IrBinOp::Shl => self.emit("    sllw t0, t1, t2"),
                        IrBinOp::AShr => self.emit("    sraw t0, t1, t2"),
                        IrBinOp::LShr => self.emit("    srlw t0, t1, t2"),
                    }
                } else {
                    match op {
                        IrBinOp::Add => self.emit("    add t0, t1, t2"),
                        IrBinOp::Sub => self.emit("    sub t0, t1, t2"),
                        IrBinOp::Mul => self.emit("    mul t0, t1, t2"),
                        IrBinOp::SDiv => self.emit("    div t0, t1, t2"),
                        IrBinOp::UDiv => self.emit("    divu t0, t1, t2"),
                        IrBinOp::SRem => self.emit("    rem t0, t1, t2"),
                        IrBinOp::URem => self.emit("    remu t0, t1, t2"),
                        IrBinOp::And => self.emit("    and t0, t1, t2"),
                        IrBinOp::Or => self.emit("    or t0, t1, t2"),
                        IrBinOp::Xor => self.emit("    xor t0, t1, t2"),
                        IrBinOp::Shl => self.emit("    sll t0, t1, t2"),
                        IrBinOp::AShr => self.emit("    sra t0, t1, t2"),
                        IrBinOp::LShr => self.emit("    srl t0, t1, t2"),
                    }
                }

                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", offset));
                }
            }
            Instruction::UnaryOp { dest, op, src, .. } => {
                self.operand_to_t0(src);
                match op {
                    IrUnaryOp::Neg => self.emit("    neg t0, t0"),
                    IrUnaryOp::Not => self.emit("    not t0, t0"),
                }
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", offset));
                }
            }
            Instruction::Cmp { dest, op, lhs, rhs, ty } => {
                self.operand_to_t0(lhs);
                self.emit("    mv t1, t0");
                self.operand_to_t0(rhs);
                self.emit("    mv t2, t0");

                // For 32-bit types, sign/zero-extend before comparison to ensure
                // correct semantics (RV64 registers are 64-bit)
                let is_32bit = *ty == IrType::I32 || *ty == IrType::U32
                    || *ty == IrType::I8 || *ty == IrType::U8
                    || *ty == IrType::I16 || *ty == IrType::U16;
                if is_32bit && ty.is_unsigned() {
                    // Zero-extend both operands to 64-bit for unsigned compare
                    self.emit("    slli t1, t1, 32");
                    self.emit("    srli t1, t1, 32");
                    self.emit("    slli t2, t2, 32");
                    self.emit("    srli t2, t2, 32");
                } else if is_32bit {
                    // Sign-extend both operands to 64-bit for signed compare
                    self.emit("    sext.w t1, t1");
                    self.emit("    sext.w t2, t2");
                }

                match op {
                    IrCmpOp::Eq => {
                        self.emit("    sub t0, t1, t2");
                        self.emit("    seqz t0, t0");
                    }
                    IrCmpOp::Ne => {
                        self.emit("    sub t0, t1, t2");
                        self.emit("    snez t0, t0");
                    }
                    IrCmpOp::Slt => self.emit("    slt t0, t1, t2"),
                    IrCmpOp::Ult => self.emit("    sltu t0, t1, t2"),
                    IrCmpOp::Sge => {
                        self.emit("    slt t0, t1, t2");
                        self.emit("    xori t0, t0, 1");
                    }
                    IrCmpOp::Uge => {
                        self.emit("    sltu t0, t1, t2");
                        self.emit("    xori t0, t0, 1");
                    }
                    IrCmpOp::Sgt => self.emit("    slt t0, t2, t1"),
                    IrCmpOp::Ugt => self.emit("    sltu t0, t2, t1"),
                    IrCmpOp::Sle => {
                        self.emit("    slt t0, t2, t1");
                        self.emit("    xori t0, t0, 1");
                    }
                    IrCmpOp::Ule => {
                        self.emit("    sltu t0, t2, t1");
                        self.emit("    xori t0, t0, 1");
                    }
                }

                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", offset));
                }
            }
            Instruction::Call { dest, func, args, .. } => {
                self.emit_call(args, Some(func), None, *dest);
            }
            Instruction::CallIndirect { dest, func_ptr, args, .. } => {
                self.emit_call(args, None, Some(func_ptr), *dest);
            }
            Instruction::GlobalAddr { dest, name } => {
                self.emit(&format!("    la t0, {}", name));
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", offset));
                }
            }
            Instruction::Copy { dest, src } => {
                self.operand_to_t0(src);
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", offset));
                }
            }
            Instruction::Cast { dest, src, from_ty, to_ty } => {
                self.operand_to_t0(src);
                self.emit_cast(*from_ty, *to_ty);
                if let Some(&offset) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", offset));
                }
            }
            Instruction::GetElementPtr { dest, base, offset, .. } => {
                if let Some(&base_off) = self.value_locations.get(&base.0) {
                    if self.alloca_values.contains(&base.0) {
                        self.emit(&format!("    addi t1, s0, {}", base_off));
                    } else {
                        self.emit(&format!("    ld t1, {}(s0)", base_off));
                    }
                }
                self.operand_to_t0(offset);
                self.emit("    add t0, t1, t0");
                if let Some(&dest_off) = self.value_locations.get(&dest.0) {
                    self.emit(&format!("    sd t0, {}(s0)", dest_off));
                }
            }
        }
    }

    /// Emit a function call (direct or indirect). Handles RISC-V calling convention.
    fn emit_call(&mut self, args: &[Operand], direct_name: Option<&str>, func_ptr: Option<&Operand>, dest: Option<Value>) {
        let arg_regs = ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"];

        for (i, arg) in args.iter().enumerate() {
            if i < 8 {
                self.operand_to_t0(arg);
                self.emit(&format!("    mv {}, t0", arg_regs[i]));
            }
        }

        if let Some(name) = direct_name {
            self.emit(&format!("    call {}", name));
        } else if let Some(ptr) = func_ptr {
            self.operand_to_t0(ptr);
            self.emit("    mv t2, t0");
            self.emit("    jalr ra, t2, 0");
        }

        if let Some(dest) = dest {
            if let Some(&offset) = self.value_locations.get(&dest.0) {
                self.emit(&format!("    sd a0, {}(s0)", offset));
            }
        }
    }

    fn generate_terminator(&mut self, term: &Terminator, frame_size: i64) {
        match term {
            Terminator::Return(val) => {
                if let Some(val) = val {
                    self.operand_to_t0(val);
                    self.emit("    mv a0, t0");
                }
                self.emit(&format!("    ld ra, {}(sp)", frame_size - 8));
                self.emit(&format!("    ld s0, {}(sp)", frame_size - 16));
                self.emit(&format!("    addi sp, sp, {}", frame_size));
                self.emit("    ret");
            }
            Terminator::Branch(label) => {
                self.emit(&format!("    j {}", label));
            }
            Terminator::CondBranch { cond, true_label, false_label } => {
                self.operand_to_t0(cond);
                self.emit(&format!("    bnez t0, {}", true_label));
                self.emit(&format!("    j {}", false_label));
            }
            Terminator::Unreachable => {
                self.emit("    ebreak");
            }
        }
    }

    fn store_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 | IrType::U8 => "sb",
            IrType::I16 | IrType::U16 => "sh",
            IrType::I32 | IrType::U32 => "sw",
            _ => "sd",
        }
    }

    fn load_for_type(ty: IrType) -> &'static str {
        match ty {
            IrType::I8 => "lb",
            IrType::U8 => "lbu",
            IrType::I16 => "lh",
            IrType::U16 => "lhu",
            IrType::I32 => "lw",
            IrType::U32 => "lwu",
            _ => "ld",
        }
    }

    /// Emit a type cast instruction sequence for RISC-V 64.
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
                    IrType::U8 => { self.emit("    andi t0, t0, 0xff"); }
                    IrType::U16 => {
                        // RISC-V doesn't have a single instruction for 16-bit zero-extend
                        // Use slli/srli to zero-extend
                        self.emit("    slli t0, t0, 48");
                        self.emit("    srli t0, t0, 48");
                    }
                    IrType::U32 => {
                        // Zero-extend 32-bit to 64-bit
                        self.emit("    slli t0, t0, 32");
                        self.emit("    srli t0, t0, 32");
                    }
                    _ => {}
                }
            } else {
                // Sign-extend for signed types
                match from_ty {
                    IrType::I8 => {
                        self.emit("    slli t0, t0, 56");
                        self.emit("    srai t0, t0, 56");
                    }
                    IrType::I16 => {
                        self.emit("    slli t0, t0, 48");
                        self.emit("    srai t0, t0, 48");
                    }
                    IrType::I32 => {
                        self.emit("    sext.w t0, t0"); // sign-extend word
                    }
                    _ => {}
                }
            }
        } else if to_size < from_size {
            // Narrowing cast: mask to target width
            match to_ty {
                IrType::I8 | IrType::U8 => { self.emit("    andi t0, t0, 0xff"); }
                IrType::I16 | IrType::U16 => {
                    self.emit("    slli t0, t0, 48");
                    self.emit("    srli t0, t0, 48");
                }
                IrType::I32 | IrType::U32 => {
                    self.emit("    sext.w t0, t0"); // truncate to 32-bit (sign-extends on RV64)
                }
                _ => {}
            }
        } else {
            // Same size: pointer <-> integer conversions (no-op on RISC-V 64)
        }
    }

    fn operand_to_t0(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => {
                match c {
                    IrConst::I8(v) => self.emit(&format!("    li t0, {}", v)),
                    IrConst::I16(v) => self.emit(&format!("    li t0, {}", v)),
                    IrConst::I32(v) => self.emit(&format!("    li t0, {}", v)),
                    IrConst::I64(v) => self.emit(&format!("    li t0, {}", v)),
                    IrConst::F32(_) | IrConst::F64(_) => self.emit("    li t0, 0"),
                    IrConst::Zero => self.emit("    li t0, 0"),
                }
            }
            Operand::Value(v) => {
                if let Some(&offset) = self.value_locations.get(&v.0) {
                    if self.alloca_values.contains(&v.0) {
                        self.emit(&format!("    addi t0, s0, {}", offset));
                    } else {
                        self.emit(&format!("    ld t0, {}(s0)", offset));
                    }
                } else {
                    self.emit("    li t0, 0");
                }
            }
        }
    }
}

impl Default for RiscvCodegen {
    fn default() -> Self {
        Self::new()
    }
}
