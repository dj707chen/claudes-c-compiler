//! RISC-V 64-bit peephole optimizer for assembly text.
//!
//! Operates on generated assembly text to eliminate redundant patterns from the
//! stack-based codegen. Processes lines in-place, marking deleted lines as empty.
//!
//! ## Optimizations
//!
//! 1. **Adjacent store/load elimination**: `sd t0, off(s0)` followed by
//!    `ld t0, off(s0)` at the same offset — the load is redundant.
//!
//! 2. **Redundant jump elimination**: `jump .LBBN, t6` (or `j .LBBN`) when
//!    `.LBBN:` is the next non-empty line — the jump is redundant.
//!
//! 3. **Self-move elimination**: `mv tX, tX` is a no-op.
//!
//! 4. **Mv chain optimization**: `mv A, B; mv C, A` → redirect second to
//!    `mv C, B`, enabling the first mv to become dead if A is unused.
//!
//! 5. **Dead store elimination**: `sd rX, off(s0)` where the slot is
//!    overwritten by another store before being read.

// ── Line classification types ────────────────────────────────────────────────

/// Compact classification of an assembly line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LineKind {
    /// Deleted / blank
    Nop,
    /// `sd reg, offset(s0)` or `sw reg, offset(s0)` — store to frame slot
    StoreS0 { reg: u8, offset: i32, is_word: bool },
    /// `ld reg, offset(s0)` or `lw reg, offset(s0)` — load from frame slot
    LoadS0 { reg: u8, offset: i32, is_word: bool },
    /// `mv rdst, rsrc` — register-to-register move
    Move { dst: u8, src: u8 },
    /// `jump .label, t6` or `j .label` — unconditional jump
    Jump,
    /// Branch instruction (beq, bne, bge, blt, bgeu, bltu)
    Branch,
    /// Label (`.LBBx:` etc.)
    Label,
    /// `ret`
    Ret,
    /// `call` or `jal ra, ...`
    Call,
    /// Assembler directive (lines starting with `.`)
    Directive,
    /// Any other instruction
    Other,
}

/// RISC-V register IDs for pattern matching.
/// We only track the registers that matter for our patterns.
const REG_NONE: u8 = 255;
const REG_T0: u8 = 0;
const REG_T1: u8 = 1;
const REG_T2: u8 = 2;
const REG_T3: u8 = 3;
const REG_T4: u8 = 4;
const REG_T5: u8 = 5;
const REG_T6: u8 = 6;
const REG_S0: u8 = 10;  // frame pointer
const REG_S1: u8 = 11;
const REG_S2: u8 = 12;
const REG_S3: u8 = 13;
const REG_S4: u8 = 14;
const REG_S5: u8 = 15;
const REG_S6: u8 = 16;
const REG_S7: u8 = 17;
const REG_S8: u8 = 18;
const REG_S9: u8 = 19;
const REG_S10: u8 = 20;
const REG_S11: u8 = 21;
const REG_A0: u8 = 30;
const REG_A1: u8 = 31;
const REG_A2: u8 = 32;
const REG_A3: u8 = 33;
const REG_A4: u8 = 34;
const REG_A5: u8 = 35;
const REG_A6: u8 = 36;
const REG_A7: u8 = 37;
const REG_RA: u8 = 40;
const REG_SP: u8 = 41;

/// Parse a register name to our internal ID.
fn parse_reg(name: &str) -> u8 {
    match name {
        "t0" => REG_T0, "t1" => REG_T1, "t2" => REG_T2, "t3" => REG_T3,
        "t4" => REG_T4, "t5" => REG_T5, "t6" => REG_T6,
        "s0" => REG_S0, "s1" => REG_S1, "s2" => REG_S2, "s3" => REG_S3,
        "s4" => REG_S4, "s5" => REG_S5, "s6" => REG_S6, "s7" => REG_S7,
        "s8" => REG_S8, "s9" => REG_S9, "s10" => REG_S10, "s11" => REG_S11,
        "a0" => REG_A0, "a1" => REG_A1, "a2" => REG_A2, "a3" => REG_A3,
        "a4" => REG_A4, "a5" => REG_A5, "a6" => REG_A6, "a7" => REG_A7,
        "ra" => REG_RA, "sp" => REG_SP,
        _ => REG_NONE,
    }
}

fn reg_name(id: u8) -> &'static str {
    match id {
        REG_T0 => "t0", REG_T1 => "t1", REG_T2 => "t2", REG_T3 => "t3",
        REG_T4 => "t4", REG_T5 => "t5", REG_T6 => "t6",
        REG_S0 => "s0", REG_S1 => "s1", REG_S2 => "s2", REG_S3 => "s3",
        REG_S4 => "s4", REG_S5 => "s5", REG_S6 => "s6", REG_S7 => "s7",
        REG_S8 => "s8", REG_S9 => "s9", REG_S10 => "s10", REG_S11 => "s11",
        REG_A0 => "a0", REG_A1 => "a1", REG_A2 => "a2", REG_A3 => "a3",
        REG_A4 => "a4", REG_A5 => "a5", REG_A6 => "a6", REG_A7 => "a7",
        REG_RA => "ra", REG_SP => "sp",
        _ => "??",
    }
}

// ── Line classification ──────────────────────────────────────────────────────

/// Classify a single assembly line into a LineKind.
fn classify_line(line: &str) -> LineKind {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineKind::Nop;
    }

    // Directives
    if trimmed.starts_with('.') {
        // Check if it's a label like `.LBB3:`
        if trimmed.ends_with(':') {
            return LineKind::Label;
        }
        return LineKind::Directive;
    }

    // Labels (e.g., `func_name:`)
    if trimmed.ends_with(':') && !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
        return LineKind::Label;
    }

    // Instructions (indented lines)
    // Store: sd/sw reg, offset(s0)
    if let Some(rest) = trimmed.strip_prefix("sd ").or_else(|| trimmed.strip_prefix("sw ")) {
        let is_word = trimmed.starts_with("sw");
        if let Some((reg_str, addr)) = rest.split_once(", ") {
            let reg = parse_reg(reg_str.trim());
            if reg != REG_NONE {
                if let Some(offset) = parse_s0_offset(addr.trim()) {
                    return LineKind::StoreS0 { reg, offset, is_word };
                }
            }
        }
    }

    // Load: ld/lw reg, offset(s0)
    if let Some(rest) = trimmed.strip_prefix("ld ").or_else(|| trimmed.strip_prefix("lw ")) {
        let is_word = trimmed.starts_with("lw");
        if let Some((reg_str, addr)) = rest.split_once(", ") {
            let reg = parse_reg(reg_str.trim());
            if reg != REG_NONE {
                if let Some(offset) = parse_s0_offset(addr.trim()) {
                    return LineKind::LoadS0 { reg, offset, is_word };
                }
            }
        }
    }

    // Move: mv dst, src
    if let Some(rest) = trimmed.strip_prefix("mv ") {
        if let Some((dst_str, src_str)) = rest.split_once(", ") {
            let dst = parse_reg(dst_str.trim());
            let src = parse_reg(src_str.trim());
            if dst != REG_NONE && src != REG_NONE {
                return LineKind::Move { dst, src };
            }
        }
    }

    // Unconditional jump: `jump .label, t6` or `j .label`
    if trimmed.starts_with("jump ") || trimmed.starts_with("j ") {
        // Don't match `jal`, `jalr`, etc.
        if trimmed.starts_with("j ") || trimmed.starts_with("jump ") {
            return LineKind::Jump;
        }
    }

    // Branch instructions
    if trimmed.starts_with("beq ") || trimmed.starts_with("bne ") ||
       trimmed.starts_with("bge ") || trimmed.starts_with("blt ") ||
       trimmed.starts_with("bgeu ") || trimmed.starts_with("bltu ") ||
       trimmed.starts_with("bnez ") || trimmed.starts_with("beqz ") {
        return LineKind::Branch;
    }

    // ret
    if trimmed == "ret" {
        return LineKind::Ret;
    }

    // call
    if trimmed.starts_with("call ") || trimmed.starts_with("jal ra,") {
        return LineKind::Call;
    }

    LineKind::Other
}

/// Parse `offset(s0)` and return the offset if it references s0.
fn parse_s0_offset(addr: &str) -> Option<i32> {
    // Format: `-24(s0)` or `48(s0)` etc.
    if !addr.ends_with("(s0)") {
        return None;
    }
    let offset_str = &addr[..addr.len() - 4]; // strip "(s0)"
    offset_str.parse::<i32>().ok()
}

/// Extract the jump target label from a jump instruction line.
fn jump_target(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("jump ") {
        // `jump .LBBN, t6` -> `.LBBN`
        if let Some((label, _)) = rest.split_once(", ") {
            return Some(label.trim());
        }
    }
    if let Some(rest) = trimmed.strip_prefix("j ") {
        return Some(rest.trim());
    }
    None
}

/// Extract the label name from a label line (strip trailing `:`)
fn label_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.ends_with(':') {
        Some(&trimmed[..trimmed.len() - 1])
    } else {
        None
    }
}

/// Check if a line references a specific s0 offset (as a read).
/// This is a conservative check for dead store elimination.
fn line_reads_offset(line: &str, offset: i32) -> bool {
    let needle = format!("{}(s0)", offset);
    line.contains(&needle)
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Run peephole optimization on RISC-V assembly text.
/// Returns the optimized assembly string.
pub fn peephole_optimize(asm: String) -> String {
    let mut lines: Vec<String> = asm.lines().map(String::from).collect();
    let mut kinds: Vec<LineKind> = lines.iter().map(|l| classify_line(l)).collect();
    let n = lines.len();

    if n == 0 {
        return asm;
    }

    // Phase 1: Iterative local passes (up to 6 rounds)
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 6 {
        changed = false;
        changed |= eliminate_adjacent_store_load(&mut lines, &mut kinds, n);
        changed |= eliminate_redundant_jumps(&lines, &mut kinds, n);
        changed |= eliminate_self_moves(&mut kinds, n);
        changed |= eliminate_redundant_mv_chain(&mut lines, &mut kinds, n);
        rounds += 1;
    }

    // Phase 2: Global passes (run once)
    // Note: global store forwarding is disabled pending correctness fixes.
    // The store-to-register tracking needs more conservative invalidation.
    let _global_changed = eliminate_dead_stores(&lines, &mut kinds, n);

    // Build result
    let mut result = String::with_capacity(asm.len());
    for i in 0..n {
        if kinds[i] != LineKind::Nop {
            result.push_str(&lines[i]);
            result.push('\n');
        }
    }
    result
}

// ── Pass 1: Adjacent store/load elimination ──────────────────────────────────
//
// Pattern: sd/sw tX, off(s0)  ;  ld/lw tX, off(s0)  (same reg, same offset)
// The load is redundant because the value is already in the register.
// Also handles: sd rX, off(s0)  ;  ld rY, off(s0)  → mv rY, rX

fn eliminate_adjacent_store_load(lines: &mut [String], kinds: &mut [LineKind], n: usize) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < n {
        if let LineKind::StoreS0 { reg: store_reg, offset: store_off, is_word: store_word } = kinds[i] {
            // Look ahead for the matching load (skip Nops)
            let mut j = i + 1;
            while j < n && kinds[j] == LineKind::Nop {
                j += 1;
            }
            if j < n {
                if let LineKind::LoadS0 { reg: load_reg, offset: load_off, is_word: load_word } = kinds[j] {
                    if store_off == load_off && store_word == load_word {
                        if store_reg == load_reg {
                            // Same register: eliminate the load
                            kinds[j] = LineKind::Nop;
                            changed = true;
                        } else {
                            // Different register: replace load with mv
                            lines[j] = format!("    mv {}, {}", reg_name(load_reg), reg_name(store_reg));
                            kinds[j] = LineKind::Move { dst: load_reg, src: store_reg };
                            changed = true;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    changed
}

// ── Pass 2: Redundant jump elimination ───────────────────────────────────────
//
// Pattern: jump .LBBN, t6  ;  .LBBN:  (jump to the immediately next label)
// The jump is redundant because execution falls through anyway.

fn eliminate_redundant_jumps(lines: &[String], kinds: &mut [LineKind], n: usize) -> bool {
    let mut changed = false;
    for i in 0..n {
        if kinds[i] == LineKind::Jump {
            // Get jump target
            if let Some(target) = jump_target(&lines[i]) {
                // Find next non-Nop line
                let mut j = i + 1;
                while j < n && kinds[j] == LineKind::Nop {
                    j += 1;
                }
                if j < n && kinds[j] == LineKind::Label {
                    if let Some(lbl) = label_name(&lines[j]) {
                        if target == lbl {
                            kinds[i] = LineKind::Nop;
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    changed
}

// ── Pass 3: Self-move elimination ────────────────────────────────────────────
//
// Pattern: mv tX, tX  (move to itself is a no-op)

fn eliminate_self_moves(kinds: &mut [LineKind], n: usize) -> bool {
    let mut changed = false;
    for i in 0..n {
        if let LineKind::Move { dst, src } = kinds[i] {
            if dst == src {
                kinds[i] = LineKind::Nop;
                changed = true;
            }
        }
    }
    changed
}

// ── Pass 4: Redundant mv chain elimination ───────────────────────────────────
//
// Pattern: mv tX, sN  ;  mv tY, tX  → mv tY, sN  (when tX is a temp not used again)
// Also: mv sN, t0  ;  mv t1, t0  → keep both (t0 is still live)
// The key insight: if we see `mv A, B` followed by `mv C, A` and A is a temp
// register, we can redirect to `mv C, B` and eliminate the first mv if A
// is not used elsewhere.

fn eliminate_redundant_mv_chain(lines: &mut [String], kinds: &mut [LineKind], n: usize) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < n {
        if let LineKind::Move { dst: dst1, src: src1 } = kinds[i] {
            // Find next non-Nop instruction
            let mut j = i + 1;
            while j < n && kinds[j] == LineKind::Nop {
                j += 1;
            }
            if j < n {
                if let LineKind::Move { dst: dst2, src: src2 } = kinds[j] {
                    // Pattern: mv dst1, src1 ; mv dst2, dst1
                    // → redirect second: mv dst2, src1
                    // Keep the first mv (dst1 might be used later).
                    if src2 == dst1 && dst2 != src1 {
                        lines[j] = format!("    mv {}, {}", reg_name(dst2), reg_name(src1));
                        kinds[j] = LineKind::Move { dst: dst2, src: src1 };
                        changed = true;
                        // The first mv may become dead. The self-move elimination or
                        // unused callee-save passes may clean it up later.
                    }
                }
            }
        }
        i += 1;
    }
    changed
}

// ── Pass 5: Dead store elimination ───────────────────────────────────────────
//
// A store to a stack slot is dead if the slot is overwritten by another store
// before being read. We look within a limited window (16 instructions).

fn eliminate_dead_stores(lines: &[String], kinds: &mut [LineKind], n: usize) -> bool {
    let mut changed = false;
    const WINDOW: usize = 16;

    for i in 0..n {
        if let LineKind::StoreS0 { offset, .. } = kinds[i] {
            // Look ahead for another store to the same offset without an
            // intervening read.
            let mut found_overwrite = false;
            let limit = (i + WINDOW).min(n);
            for j in (i + 1)..limit {
                match kinds[j] {
                    LineKind::Nop => continue,
                    LineKind::StoreS0 { offset: off2, .. } if off2 == offset => {
                        // Overwritten before read: the store at i is dead
                        found_overwrite = true;
                        break;
                    }
                    LineKind::LoadS0 { offset: off2, .. } if off2 == offset => {
                        // Read before overwrite: not dead
                        break;
                    }
                    LineKind::Label | LineKind::Branch | LineKind::Jump |
                    LineKind::Call | LineKind::Ret => {
                        // Control flow: conservatively stop
                        break;
                    }
                    _ => {
                        // Check if this instruction reads from the slot
                        if line_reads_offset(&lines[j], offset) {
                            break;
                        }
                    }
                }
            }
            if found_overwrite {
                kinds[i] = LineKind::Nop;
                changed = true;
            }
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_store() {
        assert!(matches!(
            classify_line("    sd t0, -24(s0)"),
            LineKind::StoreS0 { reg: REG_T0, offset: -24, is_word: false }
        ));
        assert!(matches!(
            classify_line("    sw a0, -32(s0)"),
            LineKind::StoreS0 { reg: REG_A0, offset: -32, is_word: true }
        ));
    }

    #[test]
    fn test_classify_load() {
        assert!(matches!(
            classify_line("    ld t0, -24(s0)"),
            LineKind::LoadS0 { reg: REG_T0, offset: -24, is_word: false }
        ));
    }

    #[test]
    fn test_classify_move() {
        assert!(matches!(
            classify_line("    mv t1, t0"),
            LineKind::Move { dst: REG_T1, src: REG_T0 }
        ));
    }

    #[test]
    fn test_classify_jump() {
        assert_eq!(classify_line("    jump .LBB3, t6"), LineKind::Jump);
        assert_eq!(classify_line("    j .LBB3"), LineKind::Jump);
    }

    #[test]
    fn test_classify_label() {
        assert_eq!(classify_line(".LBB3:"), LineKind::Label);
        assert_eq!(classify_line("main:"), LineKind::Label);
    }

    #[test]
    fn test_classify_ret() {
        assert_eq!(classify_line("    ret"), LineKind::Ret);
    }

    #[test]
    fn test_classify_branch() {
        assert_eq!(classify_line("    beq t1, t2, .LBB4"), LineKind::Branch);
        assert_eq!(classify_line("    bge t1, t2, .Lskip_0"), LineKind::Branch);
    }

    #[test]
    fn test_adjacent_store_load_elimination() {
        let input = "    sd t0, -24(s0)\n    ld t0, -24(s0)\n    ret\n";
        let result = peephole_optimize(input.to_string());
        assert!(result.contains("sd t0, -24(s0)"));
        assert!(!result.contains("ld t0, -24(s0)"));
    }

    #[test]
    fn test_redundant_jump_elimination() {
        let input = "    jump .LBB3, t6\n.LBB3:\n    ret\n";
        let result = peephole_optimize(input.to_string());
        assert!(!result.contains("jump .LBB3"));
        assert!(result.contains(".LBB3:"));
    }

    #[test]
    fn test_self_move_elimination() {
        let input = "    mv t0, t0\n    ret\n";
        let result = peephole_optimize(input.to_string());
        assert!(!result.contains("mv t0, t0"));
    }
}
