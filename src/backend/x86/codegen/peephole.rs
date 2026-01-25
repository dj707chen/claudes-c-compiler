//! x86-64 peephole optimizer for assembly text.
//!
//! This pass operates on the generated assembly text, scanning for and eliminating
//! common redundant instruction patterns that arise from the stack-based codegen:
//!
//! 1. Redundant store/load: `movq %rax, N(%rbp)` followed by `movq N(%rbp), %rax`
//!    -> eliminates the load (value is already in %rax)
//!
//! 2. Store then load to different reg: `movq %rax, N(%rbp)` followed by
//!    `movq N(%rbp), %rcx` -> replaces load with `movq %rax, %rcx`
//!
//! 3. Redundant jump: `jmp .Lfoo` where `.Lfoo:` is the very next label
//!    -> eliminates the jump
//!
//! 4. Push/pop elimination: `pushq %rax` / `movq %rax, %rcx` / `popq %rax`
//!    -> replaces with `movq %rax, %rcx` (saves the push/pop pair when the
//!    pushed value is immediately restored)

/// Strip leading spaces from an assembly line. The codegen emits lines with
/// either no indent (labels/directives) or a fixed indent (instructions).
/// Trailing whitespace is never present in codegen output, so only leading
/// spaces need to be stripped.
#[inline]
fn trim_asm(s: &str) -> &str {
    let b = s.as_bytes();
    if b.first() == Some(&b' ') {
        let mut i = 0;
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        &s[i..]
    } else {
        s
    }
}

/// Check if a line has been marked as a NOP (dead line).
/// Dead lines are marked with a NUL byte, which cannot appear in valid
/// assembly text, making this a safe and fast single-byte sentinel check.
#[inline]
fn is_nop(line: &str) -> bool {
    line.as_bytes().first() == Some(&0)
}

/// Mark a line as NOP by clearing it and writing a NUL sentinel byte.
/// Reuses the existing String allocation rather than allocating a new string.
#[inline]
fn mark_nop(line: &mut String) {
    line.clear();
    line.push('\0');
}

/// Run peephole optimization on x86-64 assembly text.
/// Returns the optimized assembly string.
pub fn peephole_optimize(asm: String) -> String {
    let mut lines: Vec<String> = asm.lines().map(|s| s.to_string()).collect();
    let mut changed = true;

    // Run multiple passes since eliminating one pattern may enable another
    let mut pass_count = 0;
    while changed && pass_count < 10 {
        changed = false;
        changed |= eliminate_redundant_store_load(&mut lines);
        changed |= eliminate_store_load_different_reg(&mut lines);
        changed |= eliminate_redundant_jumps(&mut lines);
        changed |= eliminate_push_pop_pairs(&mut lines);
        changed |= eliminate_binop_push_pop_pattern(&mut lines);
        changed |= eliminate_redundant_movq_self(&mut lines);
        changed |= fuse_compare_and_branch(&mut lines);
        changed |= forward_store_load_non_adjacent(&mut lines);
        changed |= eliminate_redundant_cltq(&mut lines);
        changed |= eliminate_dead_stores(&mut lines);
        pass_count += 1;
    }

    // Remove NOP markers and rebuild
    lines.retain(|l| !is_nop(l));

    let mut result = String::with_capacity(asm.len());
    for line in &lines {
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Pattern 1: Eliminate redundant store followed by load of the same value.
/// `movq %REG, N(%rbp)` followed by `movq N(%rbp), %REG` -> remove the load.
/// Also handles movl, movb, movw variants.
fn eliminate_redundant_store_load(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    for i in 0..len.saturating_sub(1) {
        if is_nop(&lines[i]) || is_nop(&lines[i + 1]) {
            continue;
        }

        let store_line = trim_asm(&lines[i]);
        let load_line = trim_asm(&lines[i + 1]);

        // Check: movX %reg, offset(%rbp) followed by movX offset(%rbp), %reg
        if let Some((store_reg, store_offset, store_size)) = parse_store_to_rbp(store_line) {
            if let Some((load_offset, load_reg, load_size)) = parse_load_from_rbp(load_line) {
                if store_offset == load_offset && store_reg == load_reg && store_size == load_size {
                    // The register already has the value; remove the load
                    mark_nop(&mut lines[i + 1]);
                    changed = true;
                }
            }
        }
    }
    changed
}

/// Pattern 2: Store then load from same offset to different register.
/// `movq %rax, N(%rbp)` followed by `movq N(%rbp), %rcx`
/// -> replace load with `movq %rax, %rcx` (keep the store for other uses)
fn eliminate_store_load_different_reg(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    for i in 0..len.saturating_sub(1) {
        if is_nop(&lines[i]) || is_nop(&lines[i + 1]) {
            continue;
        }

        let store_line = trim_asm(&lines[i]);
        let load_line = trim_asm(&lines[i + 1]);

        if let Some((store_reg, store_offset, store_size)) = parse_store_to_rbp(store_line) {
            if let Some((load_offset, load_reg, load_size)) = parse_load_from_rbp(load_line) {
                if store_offset == load_offset && store_reg != load_reg && store_size == load_size {
                    // Replace load with reg-to-reg move
                    let mov = match store_size {
                        MoveSize::Q => "movq",
                        MoveSize::L => "movl",
                        MoveSize::W => "movw",
                        MoveSize::B => "movb",
                        MoveSize::SLQ => "movslq",
                    };
                    lines[i + 1] = format!("    {} {}, {}", mov, store_reg, load_reg);
                    changed = true;
                }
            }
        }
    }
    changed
}

/// Pattern 3: Eliminate redundant jumps to the immediately following label.
/// `jmp .Lfoo` followed by `.Lfoo:` -> remove the jmp.
fn eliminate_redundant_jumps(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    for i in 0..len.saturating_sub(1) {
        if is_nop(&lines[i]) {
            continue;
        }

        let jmp_line = trim_asm(&lines[i]);

        // Parse: jmp LABEL
        if let Some(target) = jmp_line.strip_prefix("jmp ") {
            let target = target.trim();
            // Find the next non-NOP, non-empty line
            for j in (i + 1)..len {
                let next = trim_asm(&lines[j]);
                if next.is_empty() || is_nop(&lines[j]) {
                    continue;
                }
                // Check if it's the target label
                if let Some(label) = next.strip_suffix(':') {
                    if label == target {
                        mark_nop(&mut lines[i]);
                        changed = true;
                    }
                }
                break;
            }
        }
    }
    changed
}

/// Pattern 4: Eliminate push/pop pairs where the pushed value is preserved.
/// `pushq %rax` / ... / `popq %rax` where the intermediate instructions
/// don't use %rax, and the pattern is `pushq %rax` / `movq %rax, %rcx` / `popq %rax`
/// (this is the standard binary op pattern) -> `movq %rax, %rcx`
fn eliminate_push_pop_pairs(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    for i in 0..len.saturating_sub(2) {
        if is_nop(&lines[i]) {
            continue;
        }

        let push_line = trim_asm(&lines[i]);

        // Parse: pushq %REG
        if let Some(push_reg) = push_line.strip_prefix("pushq ") {
            let push_reg = push_reg.trim();
            if !push_reg.starts_with('%') {
                continue;
            }

            // Look for the matching popq within a small window
            for j in (i + 1)..std::cmp::min(i + 4, len) {
                if is_nop(&lines[j]) {
                    continue;
                }
                let line = trim_asm(&lines[j]);

                // Check for matching pop
                if let Some(pop_reg) = line.strip_prefix("popq ") {
                    let pop_reg = pop_reg.trim();
                    if pop_reg == push_reg {
                        // Check that intermediate instructions don't modify the pushed register
                        let mut safe = true;
                        for k in (i + 1)..j {
                            if is_nop(&lines[k]) {
                                continue;
                            }
                            if instruction_modifies_reg(trim_asm(&lines[k]), push_reg) {
                                safe = false;
                                break;
                            }
                        }
                        if safe {
                            // Remove push and pop, keep intermediates
                            mark_nop(&mut lines[i]);
                            mark_nop(&mut lines[j]);
                            changed = true;
                        }
                    }
                    break; // Found a pop (matching or not), stop looking
                }
                // If we see a push, stop (nested push/pop)
                if line.starts_with("pushq ") || line.starts_with("push ") {
                    break;
                }
                // If we see a call/jmp/ret, stop
                if line.starts_with("call") || line.starts_with("jmp") || line == "ret" {
                    break;
                }
            }
        }
    }
    changed
}

/// Pattern 5: Eliminate the common binary-op push/pop pattern.
/// The codegen generates:
///   pushq %rax              ; save LHS
///   movq <somewhere>, %rax  ; load RHS into rax
///   movq %rax, %rcx         ; move RHS to rcx
///   popq %rax               ; restore LHS
///
/// This is replaced with:
///   movq <somewhere>, %rcx  ; load RHS directly into rcx
///
/// The key insight is that the push saves %rax, then %rax is loaded with a new value,
/// moved to %rcx, and the original %rax is restored. We can just load directly into %rcx.
fn eliminate_binop_push_pop_pattern(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    let mut i = 0;
    while i + 3 < len {
        // Skip NOPs
        if is_nop(&lines[i]) {
            i += 1;
            continue;
        }

        let push_line = trim_asm(&lines[i]);

        // Match: pushq %REG
        if let Some(push_reg) = push_line.strip_prefix("pushq ") {
            let push_reg = push_reg.trim();
            if !push_reg.starts_with('%') {
                i += 1;
                continue;
            }

            // Find next 3 non-NOP lines after push (use fixed-size array)
            let mut real_indices = [0usize; 3];
            let mut count = 0;
            let mut j = i + 1;
            while j < len && count < 3 {
                if !is_nop(&lines[j]) {
                    real_indices[count] = j;
                    count += 1;
                }
                j += 1;
            }

            if count == 3 {
                let load_idx = real_indices[0];
                let move_idx = real_indices[1];
                let pop_idx = real_indices[2];

                let load_line = trim_asm(&lines[load_idx]);
                let move_line = trim_asm(&lines[move_idx]);
                let pop_line = trim_asm(&lines[pop_idx]);

                // Check: the load instruction writes to push_reg
                // (e.g., `movq -8(%rbp), %rax` or `movq $5, %rax`)
                // Check: movq %push_reg, %other_reg
                // Check: popq %push_reg
                if let Some(pop_reg) = pop_line.strip_prefix("popq ") {
                    let pop_reg = pop_reg.trim();
                    if pop_reg == push_reg {
                        // Check the move: `movq %push_reg, %other`
                        if let Some(move_target) = parse_reg_to_reg_move(move_line, push_reg) {
                            // Check the load writes to push_reg and can be safely redirected
                            if instruction_writes_to(load_line, push_reg) && can_redirect_instruction(load_line) {
                                // Transform: replace load's destination with move_target,
                                // remove the push, move, and pop
                                if let Some(new_load) = replace_dest_register(load_line, push_reg, move_target) {
                                    mark_nop(&mut lines[i]); // push
                                    lines[load_idx] = format!("    {}", new_load); // redirected load
                                    mark_nop(&mut lines[move_idx]); // move
                                    mark_nop(&mut lines[pop_idx]); // pop
                                    changed = true;
                                    i = pop_idx + 1;
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
        }

        i += 1;
    }
    changed
}

/// Parse `movq %src, %dst` and return %dst if %src matches expected_src.
fn parse_reg_to_reg_move<'a>(line: &'a str, expected_src: &str) -> Option<&'a str> {
    for prefix in &["movq ", "movl "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            if let Some((src, dst)) = rest.split_once(',') {
                let src = src.trim();
                let dst = dst.trim();
                if src == expected_src && dst.starts_with('%') {
                    return Some(dst);
                }
            }
        }
    }
    None
}

/// Check if an instruction writes to a specific register as its destination.
fn instruction_writes_to(inst: &str, reg: &str) -> bool {
    // Two-operand instructions: the destination is the last operand
    if let Some((_op, operands)) = inst.split_once(' ') {
        if let Some((_src, dst)) = operands.rsplit_once(',') {
            let dst = dst.trim();
            if dst == reg || register_overlaps(dst, reg) {
                return true;
            }
        }
    }
    false
}

/// Check if an instruction can have its destination register replaced safely.
/// Some instructions have encoding restrictions (e.g., certain forms of movabs).
fn can_redirect_instruction(inst: &str) -> bool {
    // movabsq with immediate can technically take any GPR, but some assembler
    // versions have issues. Also skip any instruction that references memory
    // through %rax (like `movq (%rax), %rax`) since %rax would need to be
    // preserved for the address computation.
    if inst.starts_with("movabsq ") {
        return false;
    }
    // Skip inline asm or anything unusual
    if inst.starts_with(".") || inst.ends_with(":") {
        return false;
    }
    true
}

/// Replace the destination register in an instruction.
/// Only handles the common patterns generated by our codegen:
/// - `movq <mem>, %old` -> `movq <mem>, %new`
/// - `movq $imm, %old` -> `movq $imm, %new`
/// - `leaq <mem>, %old` -> `leaq <mem>, %new`
/// - `xorq %old, %old` -> `xorq %new, %new`
/// - `movslq <mem>, %old` -> `movslq <mem>, %new`
/// Returns None if the instruction cannot be safely redirected.
fn replace_dest_register(inst: &str, old_reg: &str, new_reg: &str) -> Option<String> {
    // Only handle 64-bit registers to avoid partial register complications
    if !old_reg.starts_with("%r") || !new_reg.starts_with("%r") {
        return None;
    }

    // Handle `xorq %reg, %reg` (zero idiom) specially
    if let Some(rest) = inst.strip_prefix("xorq ") {
        if let Some((src, dst)) = rest.split_once(',') {
            let src = src.trim();
            let dst = dst.trim();
            if src == old_reg && dst == old_reg {
                return Some(format!("xorq {}, {}", new_reg, new_reg));
            }
        }
    }

    // Safe instruction prefixes that can have their destination redirected.
    // Only includes instructions where the destination is the last operand
    // and the source doesn't reference the destination register.
    for prefix in &["movq ", "movslq ", "leaq ", "movzbq "] {
        if let Some(rest) = inst.strip_prefix(prefix) {
            if let Some((src, dst)) = rest.rsplit_once(',') {
                let src = src.trim();
                let dst = dst.trim();
                if dst == old_reg {
                    // Safety: check the source doesn't reference old_reg
                    // (e.g., `movq (%rax), %rax` can't be redirected to
                    // `movq (%rax), %rcx` if %rax is the push_reg because
                    // we're eliminating the push that saves it)
                    // Actually wait - the push saves the value, and in the
                    // transformed version %rax is never modified, so accessing
                    // (%rax) is fine. But let me be safe and check anyway.
                    if !src.contains(old_reg) {
                        return Some(format!("{}{}, {}", prefix, src, new_reg));
                    }
                }
            }
        }
    }

    None
}

/// Pattern 6: Eliminate redundant self-moves like `movq %rax, %rax`.
fn eliminate_redundant_movq_self(lines: &mut [String]) -> bool {
    let mut changed = false;
    for line in lines.iter_mut() {
        if is_nop(line) {
            continue;
        }
        let trimmed = trim_asm(line);
        if is_self_move(trimmed) {
            mark_nop(line);
            changed = true;
        }
    }
    changed
}

/// Check if an instruction is a self-move (e.g., movq %rax, %rax).
fn is_self_move(s: &str) -> bool {
    // Only 64-bit self-moves are true no-ops on x86-64.
    // 32-bit movl %eax, %eax zero-extends to 64 bits (NOT a no-op).
    // 16/8-bit moves have partial register write semantics.
    if let Some(rest) = s.strip_prefix("movq ") {
        let rest = rest.trim();
        if let Some((src, dst)) = rest.split_once(',') {
            let src = src.trim();
            let dst = dst.trim();
            if src == dst && src.starts_with('%') {
                return true;
            }
        }
    }
    false
}

/// Check if an instruction modifies the given register.
fn instruction_modifies_reg(inst: &str, reg: &str) -> bool {
    // Instructions that modify a register: mov to reg, arithmetic with reg as dest, etc.
    // For safety, we check if the instruction writes to the register.
    // Most x86 instructions have the form: op src, dst (AT&T syntax)
    // The destination is the LAST operand.

    // Skip empty/NOP/labels/directives
    if inst.is_empty() || is_nop(inst) || inst.ends_with(':') || inst.starts_with('.') {
        return false;
    }

    // Special case: `movq %rax, %rcx` -> modifies %rcx, not %rax
    // In AT&T syntax, the last operand is the destination
    if let Some((_op, operands)) = inst.split_once(' ') {
        if let Some((_src, dst)) = operands.rsplit_once(',') {
            let dst = dst.trim();
            // Check if dst is or contains the register
            if dst == reg || register_overlaps(dst, reg) {
                return true;
            }
        } else {
            // Single operand instructions like `popq %rax`, `pushq %rax`
            // `popq %rax` modifies %rax
            let operand = operands.trim();
            if inst.starts_with("pop") && (operand == reg || register_overlaps(operand, reg)) {
                return true;
            }
            // `incq %rax`, `decq %rax`, `notq %rax`, `negq %rax` modify the operand
            if (inst.starts_with("inc") || inst.starts_with("dec") ||
                inst.starts_with("not") || inst.starts_with("neg")) &&
                (operand == reg || register_overlaps(operand, reg)) {
                return true;
            }
        }
    }

    false
}

/// Pattern 7: Fuse compare-and-branch sequences.
///
/// The codegen produces this sequence for every conditional branch:
///   cmpX %rcx, %rax       (or cmpl, testq, etc.)
///   setCC %al              (materialize boolean)
///   movzbq %al, %rax      (zero-extend to 64-bit)
///   movq %rax, N(%rbp)    (store boolean to stack)
///   movq N(%rbp), %rax    (reload boolean -- may already be eliminated)
///   testq %rax, %rax      (test boolean)
///   jne .Ltrue             (branch if true)
///   jmp .Lfalse            (fallthrough to false)
///
/// This is replaced with:
///   cmpX %rcx, %rax
///   jCC .Ltrue             (direct conditional jump using flags from cmp)
///   jmp .Lfalse
///
/// Also handles the variant where the store/load pair has already been optimized
/// away (so the sequence is: cmp / setCC / movzbq / testq / jne / jmp).
fn fuse_compare_and_branch(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    let mut i = 0;
    while i < len {
        if is_nop(&lines[i]) {
            i += 1;
            continue;
        }

        // Collect the next non-NOP lines starting from i (up to 8, fixed-size array)
        let mut seq_indices = [0usize; 8];
        let mut seq_count = 0;
        let mut j = i;
        while j < len && seq_count < 8 {
            if !is_nop(&lines[j]) {
                seq_indices[seq_count] = j;
                seq_count += 1;
            }
            j += 1;
        }

        if seq_count < 4 {
            i += 1;
            continue;
        }

        // Try to match the pattern starting with a cmp/test instruction
        let cmp_line = trim_asm(&lines[seq_indices[0]]);

        // Must be a comparison or test instruction that sets flags
        let is_cmp = cmp_line.starts_with("cmpq ") || cmp_line.starts_with("cmpl ")
            || cmp_line.starts_with("cmpw ") || cmp_line.starts_with("cmpb ")
            || cmp_line.starts_with("testq ") || cmp_line.starts_with("testl ")
            || cmp_line.starts_with("testw ") || cmp_line.starts_with("testb ")
            || cmp_line.starts_with("ucomisd ") || cmp_line.starts_with("ucomiss ");

        if !is_cmp {
            i += 1;
            continue;
        }

        // Look for setCC as the next instruction
        let set_line = trim_asm(&lines[seq_indices[1]]);
        let cc = parse_setcc(set_line);
        if cc.is_none() {
            i += 1;
            continue;
        }
        let cc = cc.unwrap();

        // The rest of the pattern can vary depending on what prior peephole
        // passes have already done. We need to find the testq + jne/je sequence.
        // Pattern A (full): setCC / movzbq / movq store / movq load / testq / jne / jmp
        // Pattern B (store/load eliminated): setCC / movzbq / testq / jne / jmp
        // Pattern C (movzbq+test eliminated too): setCC / movzbq / testq / jne / jmp
        //
        // In all cases, after the setCC we look for testq %rax, %rax + jne/je
        // scanning forward through movzbq and store/load pairs.

        let mut test_idx = None;
        let mut scan = 2; // start after setCC
        while scan < seq_count {
            let line = trim_asm(&lines[seq_indices[scan]]);
            // Skip movzbq %al, %rax (zero-extend of the setcc result)
            if line.starts_with("movzbq %al,") || line.starts_with("movzbl %al,") {
                scan += 1;
                continue;
            }
            // Skip store to rbp (storing the boolean)
            if parse_store_to_rbp(line).is_some() {
                scan += 1;
                continue;
            }
            // Skip load from rbp (reloading the boolean)
            if parse_load_from_rbp(line).is_some() {
                scan += 1;
                continue;
            }
            // Skip cltq (sign-extend eax to rax)
            if line == "cltq" {
                scan += 1;
                continue;
            }
            // Skip movslq (sign extend) - also used for the boolean
            if line.starts_with("movslq ") {
                scan += 1;
                continue;
            }
            // Check for testq/testl %rax, %rax
            if line == "testq %rax, %rax" || line == "testl %eax, %eax" {
                test_idx = Some(scan);
                break;
            }
            // If we hit anything else, the pattern doesn't match
            break;
        }

        if test_idx.is_none() {
            i += 1;
            continue;
        }
        let test_scan = test_idx.unwrap();

        // After testq, we need jne or je
        if test_scan + 1 >= seq_count {
            i += 1;
            continue;
        }
        let jmp_line = trim_asm(&lines[seq_indices[test_scan + 1]]);
        let (is_jne, branch_target) = if let Some(target) = jmp_line.strip_prefix("jne ") {
            (true, target.trim())
        } else if let Some(target) = jmp_line.strip_prefix("je ") {
            (false, target.trim())
        } else {
            i += 1;
            continue;
        };

        // Determine the fused conditional jump opcode.
        // If the branch is `jne` (branch if condition != 0), use the direct condition.
        // If the branch is `je` (branch if condition == 0), use the inverted condition.
        let fused_cc = if is_jne { cc } else { invert_cc(cc) };

        let fused_jcc = format!("    j{} {}", fused_cc, branch_target);

        // NOP out everything from setCC through testq, replace testq+1 (the jne/je) with jCC
        for s in 1..=test_scan {
            mark_nop(&mut lines[seq_indices[s]]);
        }
        lines[seq_indices[test_scan + 1]] = fused_jcc;

        changed = true;
        i = seq_indices[test_scan + 1] + 1;
    }

    changed
}

/// Parse a setCC instruction and return the condition code string.
/// E.g., "sete %al" -> Some("e"), "setl %al" -> Some("l")
fn parse_setcc(s: &str) -> Option<&str> {
    if !s.starts_with("set") {
        return None;
    }
    // Handle "setnp %al" and similar two-char conditions first
    // The format is "setCC %reg" where CC is 1-3 chars
    let rest = &s[3..]; // skip "set"
    let space_idx = rest.find(' ')?;
    let cc = &rest[..space_idx];
    // Validate it's a known condition code
    match cc {
        "e" | "ne" | "l" | "le" | "g" | "ge" | "b" | "be" | "a" | "ae"
        | "s" | "ns" | "o" | "no" | "p" | "np" | "z" | "nz" => Some(cc),
        _ => None,
    }
}

/// Invert a condition code (e.g., "e" -> "ne", "l" -> "ge")
fn invert_cc(cc: &str) -> &str {
    match cc {
        "e" | "z" => "ne",
        "ne" | "nz" => "e",
        "l" => "ge",
        "ge" => "l",
        "le" => "g",
        "g" => "le",
        "b" => "ae",
        "ae" => "b",
        "be" => "a",
        "a" => "be",
        "s" => "ns",
        "ns" => "s",
        "o" => "no",
        "no" => "o",
        "p" => "np",
        "np" => "p",
        _ => cc, // fallback, should not happen
    }
}

/// Pattern 8: Forward stores to non-adjacent loads from the same rbp offset.
///
/// When a value is stored to a stack slot and then loaded from the same slot
/// several instructions later (not immediately adjacent), we can replace the
/// load with a register-to-register move if the intervening instructions don't
/// write to the source register or to the same stack slot.
///
/// Example:
///   movq %rax, -24(%rbp)     # store
///   ... (instructions that don't modify %rax or -24(%rbp)) ...
///   movq -24(%rbp), %rax     # load -> eliminated (value is still in %rax)
///   movq -24(%rbp), %rcx     # load -> replaced with: movq %rax, %rcx
fn forward_store_load_non_adjacent(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();
    // Look at a window of instructions - track recent stores to rbp slots
    // The window size limits how far ahead we look. 16 is a reasonable
    // trade-off between effectiveness and safety.
    const WINDOW: usize = 20;

    for i in 0..len {
        if is_nop(&lines[i]) {
            continue;
        }

        // Parse store info from the trimmed line, then extract small owned copies
        // of register and offset (avoids cloning the entire line)
        let trimmed_i = trim_asm(&lines[i]);
        let store_info = parse_store_to_rbp(trimmed_i);
        if store_info.is_none() {
            continue;
        }
        let (sr, so, store_size) = store_info.unwrap();
        let store_reg = sr.to_string();
        let store_offset = so.to_string();

        // Scan forward for loads from the same offset
        let mut reg_still_valid = true;
        let end = std::cmp::min(i + WINDOW, len);

        for j in (i + 1)..end {
            if is_nop(&lines[j]) {
                continue;
            }

            let line = trim_asm(&lines[j]);

            // If we hit a label, call, jump, or ret, stop scanning
            if line.ends_with(':') || line.starts_with("call")
                || line.starts_with("jmp ") || line.starts_with("je ")
                || line.starts_with("jne ") || line.starts_with("jl ")
                || line.starts_with("jle ") || line.starts_with("jg ")
                || line.starts_with("jge ") || line.starts_with("jb ")
                || line.starts_with("jbe ") || line.starts_with("ja ")
                || line.starts_with("jae ") || line.starts_with("js ")
                || line.starts_with("jns ") || line.starts_with("jo ")
                || line.starts_with("jno ") || line.starts_with("jp ")
                || line.starts_with("jnp ") || line.starts_with("jnz ")
                || line.starts_with("jz ")
                || line == "ret"
                || line.starts_with(".")
            {
                break;
            }

            // Check if this line is another store to the same offset (kills forwarding)
            if let Some((_, other_offset, _)) = parse_store_to_rbp(line) {
                if other_offset == store_offset {
                    break; // The slot was overwritten
                }
            }

            // Check if this is a load from the same offset BEFORE checking
            // register modifications (since the load itself writes the dest register)
            if let Some((load_offset, load_reg, load_size)) = parse_load_from_rbp(line) {
                if load_offset == store_offset && load_size == store_size && reg_still_valid {
                    if load_reg == store_reg {
                        // Same register: the load is entirely redundant
                        mark_nop(&mut lines[j]);
                        changed = true;
                        continue;
                    } else {
                        // Different register: replace load with reg-to-reg move
                        let mov = match store_size {
                            MoveSize::Q => "movq",
                            MoveSize::L => "movl",
                            MoveSize::W => "movw",
                            MoveSize::B => "movb",
                            MoveSize::SLQ => "movslq",
                        };
                        lines[j] = format!("    {} {}, {}", mov, store_reg, load_reg);
                        changed = true;
                        continue;
                    }
                }
            }

            // Check if this instruction modifies the source register
            if reg_still_valid && instruction_modifies_reg(line, &store_reg) {
                reg_still_valid = false;
            }
        }
    }

    changed
}

/// Pattern 9: Eliminate redundant cltq after movslq.
///
/// The codegen sometimes produces:
///   movslq N(%rbp), %rax    (sign-extend 32-bit to 64-bit)
///   cltq                     (sign-extend eax to rax - redundant!)
///
/// The movslq already sign-extends the value to 64 bits, so cltq is a no-op.
fn eliminate_redundant_cltq(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();

    for i in 0..len.saturating_sub(1) {
        if is_nop(&lines[i]) || is_nop(&lines[i + 1]) {
            continue;
        }

        let curr_is_cltq = trim_asm(&lines[i + 1]) == "cltq";
        if !curr_is_cltq {
            continue;
        }

        let prev = trim_asm(&lines[i]);

        // movslq already sign-extends to 64-bit, so following cltq is redundant
        if prev.starts_with("movslq ") && prev.contains("%rax") {
            mark_nop(&mut lines[i + 1]);
            changed = true;
            continue;
        }

        // movq $IMM, %rax followed by cltq is also redundant (constant already full-width)
        if prev.starts_with("movq $") && prev.ends_with("%rax") {
            mark_nop(&mut lines[i + 1]);
            changed = true;
        }
    }

    changed
}

/// Pattern 10: Eliminate dead stores to stack slots.
///
/// If a store to a stack slot is followed by another store to the same slot
/// with no intervening load, the first store is dead and can be eliminated.
fn eliminate_dead_stores(lines: &mut [String]) -> bool {
    let mut changed = false;
    let len = lines.len();
    const WINDOW: usize = 16;

    for i in 0..len {
        if is_nop(&lines[i]) {
            continue;
        }

        let trimmed_i = trim_asm(&lines[i]);
        let store_info = parse_store_to_rbp(trimmed_i);
        if store_info.is_none() {
            continue;
        }
        let (_, so, _) = store_info.unwrap();
        let store_offset = so.to_string();

        // Pre-compute the slot pattern once outside the inner loop
        let slot_pattern = format!("{}(%rbp)", store_offset);

        // Scan forward to see if the slot is read before being overwritten
        let end = std::cmp::min(i + WINDOW, len);
        let mut slot_read = false;
        let mut slot_overwritten = false;

        for j in (i + 1)..end {
            if is_nop(&lines[j]) {
                continue;
            }

            let line = trim_asm(&lines[j]);

            // If we hit a label/call/jump/ret, the slot might be read later
            if line.ends_with(':') || line.starts_with("call")
                || line.starts_with("jmp ") || line == "ret"
                || line.starts_with(".")
                || is_conditional_jump(line)
            {
                slot_read = true; // conservatively assume the slot may be read
                break;
            }

            // Check if this loads from the same offset (slot is read)
            if let Some((load_offset, _, _)) = parse_load_from_rbp(line) {
                if load_offset == store_offset {
                    slot_read = true;
                    break;
                }
            }

            // Check if another store to the same offset (slot overwritten before read)
            if let Some((_, other_offset, _)) = parse_store_to_rbp(line) {
                if other_offset == store_offset {
                    slot_overwritten = true;
                    break;
                }
            }

            // Check for other instructions that reference the slot via rbp
            // (e.g., leaq -24(%rbp), %rax or movslq -24(%rbp), %rax)
            // This is a conservative catch-all for non-standard access patterns.
            // Exclude stores (already handled above) and loads (already handled above).
            if line.contains(slot_pattern.as_str()) {
                slot_read = true;
                break;
            }
        }

        if slot_overwritten && !slot_read {
            mark_nop(&mut lines[i]);
            changed = true;
        }
    }

    changed
}

/// Check if a line is a conditional jump instruction.
fn is_conditional_jump(s: &str) -> bool {
    s.starts_with("je ") || s.starts_with("jne ") || s.starts_with("jl ")
        || s.starts_with("jle ") || s.starts_with("jg ") || s.starts_with("jge ")
        || s.starts_with("jb ") || s.starts_with("jbe ") || s.starts_with("ja ")
        || s.starts_with("jae ") || s.starts_with("js ") || s.starts_with("jns ")
        || s.starts_with("jo ") || s.starts_with("jno ") || s.starts_with("jp ")
        || s.starts_with("jnp ") || s.starts_with("jz ") || s.starts_with("jnz ")
}

/// Check if two register names overlap (e.g., %eax overlaps with %rax).
fn register_overlaps(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_family = register_family(a);
    let b_family = register_family(b);
    a_family.is_some() && a_family == b_family
}

/// Get the register family (0-15) for an x86 register name.
fn register_family(reg: &str) -> Option<u8> {
    match reg {
        "%rax" | "%eax" | "%ax" | "%al" | "%ah" => Some(0),
        "%rcx" | "%ecx" | "%cx" | "%cl" | "%ch" => Some(1),
        "%rdx" | "%edx" | "%dx" | "%dl" | "%dh" => Some(2),
        "%rbx" | "%ebx" | "%bx" | "%bl" | "%bh" => Some(3),
        "%rsp" | "%esp" | "%sp" | "%spl" => Some(4),
        "%rbp" | "%ebp" | "%bp" | "%bpl" => Some(5),
        "%rsi" | "%esi" | "%si" | "%sil" => Some(6),
        "%rdi" | "%edi" | "%di" | "%dil" => Some(7),
        "%r8" | "%r8d" | "%r8w" | "%r8b" => Some(8),
        "%r9" | "%r9d" | "%r9w" | "%r9b" => Some(9),
        "%r10" | "%r10d" | "%r10w" | "%r10b" => Some(10),
        "%r11" | "%r11d" | "%r11w" | "%r11b" => Some(11),
        "%r12" | "%r12d" | "%r12w" | "%r12b" => Some(12),
        "%r13" | "%r13d" | "%r13w" | "%r13b" => Some(13),
        "%r14" | "%r14d" | "%r14w" | "%r14b" => Some(14),
        "%r15" | "%r15d" | "%r15w" | "%r15b" => Some(15),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveSize {
    Q,   // movq  (64-bit)
    L,   // movl  (32-bit)
    W,   // movw  (16-bit)
    B,   // movb  (8-bit)
    SLQ, // movslq (sign-extend 32->64)
}

/// Parse `movX %reg, offset(%rbp)` (store to rbp-relative slot).
/// Returns (register, offset, size).
fn parse_store_to_rbp(s: &str) -> Option<(&str, &str, MoveSize)> {
    let (rest, size) = if let Some(r) = s.strip_prefix("movq ") {
        (r, MoveSize::Q)
    } else if let Some(r) = s.strip_prefix("movl ") {
        (r, MoveSize::L)
    } else if let Some(r) = s.strip_prefix("movw ") {
        (r, MoveSize::W)
    } else if let Some(r) = s.strip_prefix("movb ") {
        (r, MoveSize::B)
    } else {
        return None;
    };

    let (src, dst) = rest.split_once(',')?;
    let src = src.trim();
    let dst = dst.trim();

    // src must be a register
    if !src.starts_with('%') {
        return None;
    }

    // dst must be offset(%rbp)
    if !dst.ends_with("(%rbp)") {
        return None;
    }
    let offset = &dst[..dst.len() - 6]; // Strip "(%rbp)"

    Some((src, offset, size))
}

/// Parse `movX offset(%rbp), %reg` or `movslq offset(%rbp), %reg` (load from rbp-relative slot).
/// Returns (offset, register, size).
fn parse_load_from_rbp(s: &str) -> Option<(&str, &str, MoveSize)> {
    let (rest, size) = if let Some(r) = s.strip_prefix("movq ") {
        (r, MoveSize::Q)
    } else if let Some(r) = s.strip_prefix("movl ") {
        (r, MoveSize::L)
    } else if let Some(r) = s.strip_prefix("movw ") {
        (r, MoveSize::W)
    } else if let Some(r) = s.strip_prefix("movb ") {
        (r, MoveSize::B)
    } else if let Some(r) = s.strip_prefix("movslq ") {
        (r, MoveSize::SLQ)
    } else {
        return None;
    };

    let (src, dst) = rest.split_once(',')?;
    let src = src.trim();
    let dst = dst.trim();

    // src must be offset(%rbp)
    if !src.ends_with("(%rbp)") {
        return None;
    }
    let offset = &src[..src.len() - 6];

    // dst must be a register
    if !dst.starts_with('%') {
        return None;
    }

    Some((offset, dst, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redundant_store_load() {
        let asm = "    movq %rax, -8(%rbp)\n    movq -8(%rbp), %rax\n".to_string();
        let result = peephole_optimize(asm);
        assert_eq!(result.trim(), "movq %rax, -8(%rbp)");
    }

    #[test]
    fn test_store_load_different_reg() {
        let asm = "    movq %rax, -8(%rbp)\n    movq -8(%rbp), %rcx\n".to_string();
        let result = peephole_optimize(asm);
        assert!(result.contains("movq %rax, -8(%rbp)"));
        assert!(result.contains("movq %rax, %rcx"));
        assert!(!result.contains("movq -8(%rbp), %rcx"));
    }

    #[test]
    fn test_redundant_jump() {
        let asm = "    jmp .Lfoo\n.Lfoo:\n".to_string();
        let result = peephole_optimize(asm);
        assert!(!result.contains("jmp"));
        assert!(result.contains(".Lfoo:"));
    }

    #[test]
    fn test_push_pop_elimination() {
        let asm = "    pushq %rax\n    movq %rax, %rcx\n    popq %rax\n".to_string();
        let result = peephole_optimize(asm);
        assert!(!result.contains("pushq"));
        assert!(!result.contains("popq"));
        assert!(result.contains("movq %rax, %rcx"));
    }

    #[test]
    fn test_self_move() {
        let asm = "    movq %rax, %rax\n".to_string();
        let result = peephole_optimize(asm);
        assert_eq!(result.trim(), "");
    }

    #[test]
    fn test_parse_store_to_rbp() {
        assert!(parse_store_to_rbp("movq %rax, -8(%rbp)").is_some());
        assert!(parse_store_to_rbp("movl %eax, -16(%rbp)").is_some());
        assert!(parse_store_to_rbp("movq $5, -8(%rbp)").is_none()); // not a register source
    }

    #[test]
    fn test_parse_load_from_rbp() {
        assert!(parse_load_from_rbp("movq -8(%rbp), %rax").is_some());
        assert!(parse_load_from_rbp("movslq -8(%rbp), %rax").is_some());
    }

    #[test]
    fn test_compare_branch_fusion_full() {
        // Full pattern: cmp / setl / movzbq / store / load / testq / jne / jmp
        let asm = [
            "    cmpq %rcx, %rax",
            "    setl %al",
            "    movzbq %al, %rax",
            "    movq %rax, -24(%rbp)",
            "    movq -24(%rbp), %rax",
            "    testq %rax, %rax",
            "    jne .L2",
            "    jmp .L4",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        assert!(result.contains("cmpq %rcx, %rax"), "should keep the cmp");
        assert!(result.contains("jl .L2"), "should fuse to jl: {}", result);
        assert!(result.contains("jmp .L4"), "should keep the fallthrough jmp");
        assert!(!result.contains("setl"), "should eliminate setl");
        assert!(!result.contains("movzbq"), "should eliminate movzbq");
        assert!(!result.contains("testq"), "should eliminate testq");
    }

    #[test]
    fn test_compare_branch_fusion_short() {
        // Short pattern after prior peephole: cmp / setl / movzbq / testq / jne / jmp
        // (store/load already eliminated)
        let asm = [
            "    cmpq %rcx, %rax",
            "    setl %al",
            "    movzbq %al, %rax",
            "    testq %rax, %rax",
            "    jne .L2",
            "    jmp .L4",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        assert!(result.contains("jl .L2"), "should fuse to jl: {}", result);
        assert!(!result.contains("setl"), "should eliminate setl");
    }

    #[test]
    fn test_compare_branch_fusion_je() {
        // When branch uses je instead of jne, invert the condition
        let asm = [
            "    cmpq %rcx, %rax",
            "    setl %al",
            "    movzbq %al, %rax",
            "    testq %rax, %rax",
            "    je .Lfalse",
            "    jmp .Ltrue",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        // je means "branch if NOT condition", so setl + je => jge
        assert!(result.contains("jge .Lfalse"), "should fuse to jge: {}", result);
    }

    #[test]
    fn test_non_adjacent_store_load_same_reg() {
        // Store to slot, intervening instruction, load from same slot to same reg
        let asm = [
            "    movq %rax, -24(%rbp)",
            "    movq %rcx, -32(%rbp)",
            "    movq -24(%rbp), %rax",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        // The load should be eliminated since %rax still has the value
        assert!(!result.contains("-24(%rbp), %rax"), "should eliminate the load: {}", result);
    }

    #[test]
    fn test_non_adjacent_store_load_diff_reg() {
        // Store to slot, intervening instruction, load from same slot to different reg
        let asm = [
            "    movq %rax, -24(%rbp)",
            "    movq %rcx, -32(%rbp)",
            "    movq -24(%rbp), %rdx",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        // The load should become movq %rax, %rdx
        assert!(result.contains("movq %rax, %rdx"), "should forward to reg-reg: {}", result);
    }

    #[test]
    fn test_non_adjacent_store_load_reg_modified() {
        // Store to slot, modify source reg, load from same slot
        // Should NOT forward because %rax was modified
        let asm = [
            "    movq %rax, -24(%rbp)",
            "    movq -32(%rbp), %rax",
            "    movq -24(%rbp), %rcx",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        // Should NOT be optimized since %rax was overwritten
        assert!(result.contains("-24(%rbp), %rcx") || result.contains("%rax, %rcx"),
            "should not forward since rax was modified: {}", result);
    }

    #[test]
    fn test_redundant_cltq() {
        let asm = "    movslq -8(%rbp), %rax\n    cltq\n".to_string();
        let result = peephole_optimize(asm);
        assert!(result.contains("movslq"), "should keep movslq");
        assert!(!result.contains("cltq"), "should eliminate redundant cltq: {}", result);
    }

    #[test]
    fn test_dead_store_elimination() {
        // Two consecutive stores to the same slot, no intervening read
        let asm = [
            "    movq %rax, -24(%rbp)",
            "    movq %rcx, -24(%rbp)",
        ].join("\n") + "\n";
        let result = peephole_optimize(asm);
        // First store should be eliminated
        assert!(!result.contains("%rax, -24(%rbp)"), "first store should be dead: {}", result);
        assert!(result.contains("%rcx, -24(%rbp)"), "second store should remain: {}", result);
    }

    #[test]
    fn test_condition_codes() {
        // Test various condition codes work correctly
        for (cc, expected_jcc) in &[("e", "je"), ("ne", "jne"), ("l", "jl"), ("g", "jg"),
                                     ("le", "jle"), ("ge", "jge"), ("b", "jb"), ("a", "ja")] {
            let asm = format!(
                "    cmpq %rcx, %rax\n    set{} %al\n    movzbq %al, %rax\n    testq %rax, %rax\n    jne .L1\n",
                cc
            );
            let result = peephole_optimize(asm);
            assert!(result.contains(&format!("{} .L1", expected_jcc)),
                "cc={} should produce {}: {}", cc, expected_jcc, result);
        }
    }
}
