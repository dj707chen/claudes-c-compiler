Reviewing mem2reg README for accuracy. Found one factual issue:
the file inventory says "Both passes" depend on ir::analysis, but
only promote.rs uses ir::analysis; phi_eliminate.rs is self-contained.
