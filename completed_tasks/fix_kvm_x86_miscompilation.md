Task: Fix cross-block copy alias causing stack slot collision with inline asm
Status: Complete
Branch: master

Problem:
The kernel's arch/x86/kernel/kvm.c miscompiled, causing a NULL pointer
dereference in hypervisor_cpuid_base() during early boot (zero serial
output from QEMU). The CPUID inline assembly's output pointer for the
edx register was overwritten by an unrelated temporary value before the
asm output store phase could read it.

Root cause:
After inlining native_cpuid() into hypervisor_cpuid_base(), the inline
asm output pointer for "=d" (*edx) was a GEP value (v20 = &signature[2])
defined in the entry block. After mem2reg, a Copy instruction (v86 = Copy
v20) was created in the entry block, and v86 was used as the output
pointer in the inline asm in the loop body (block 3).

The copy coalescing optimization aliased v86 to v20 (same stack slot),
but v20's liveness interval only covered the entry block [9, 10]. The
Tier 2 interval packing saw this short interval and reused v20's slot
(-176(%rbp)) for another value (v61, a loop variable) with interval
[16, 56]. When the loop body executed, v61 overwrote the pointer stored
at -176(%rbp), and the inline asm Phase 4 output store dereferenced
the corrupted value as a pointer, causing a NULL pointer dereference.

Additionally, even without Tier 2 reuse, the Tier 3 block-local
classification would assign v20 to the block-0 pool. Block 3's pool
shares the same physical stack offsets, so block-3-local temporaries
(the inline asm input loads) could also overwrite the pointer.

Diagnosis:
- Delta debugging identified arch/x86/kernel/kvm.c as the single
  failing file
- QEMU -d int,cpu_reset traced crash to hypervisor_cpuid_base+0x10b
- Page fault CR2=0x0 (NULL deref) after CPUID instruction
- Disassembly showed -176(%rbp) used for both the edx output pointer
  address and the eax input value temporary
- Minimal reproduction: always_inline CPUID wrapper with pointer outputs
  in a loop, segfaults with ccc -O2 but works with gcc

Fix:
Two changes to src/backend/stack_layout.rs:
1. Skip copy aliasing when the dest value has uses in blocks different
   from the source's definition block. This prevents the root's short
   liveness interval from being shared with a value that needs the slot
   to survive across blocks.
2. Propagate copy-alias dest uses into the root's use_blocks_map, so
   coalescable_group correctly classifies aliased roots as multi-block
   values (Tier 2) rather than block-local (Tier 3).

Files changed:
- src/backend/stack_layout.rs: Added cross-block alias check in copy
  coalescing, added use_blocks_map propagation after alias construction,
  moved coalescable_group closure after propagation
- tests/asm-output-ptr-cross-block-slot-x86/: Regression test with
  always_inline CPUID wrapper exercising the cross-block output pointer
  pattern
