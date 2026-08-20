/* Kendryte K210 — Sipeed Maix Go.

   The K210 has no internal flash. Its mask ROM reads the image out of the
   board's SPI NOR part, copies it to 0x80000000 and jumps there, so this is a
   RAM-only layout: text, rodata, data, bss, heap and stack all live in SRAM
   and there is no load-address/run-address split to arrange.

   6 MB, not 8. The general-purpose SRAM is two banks — 4 MB at 0x80000000 and
   2 MB at 0x80400000 — which are contiguous and so described as one region.
   The 2 MB above that, at 0x80600000, is the KPU's AI RAM: usable as ordinary
   memory only after the AI clock domain is ungated, so it is deliberately left
   out rather than handed to the linker on trust. */
MEMORY
{
  RAM : ORIGIN = 0x80000000, LENGTH = 6M
}

REGION_ALIAS("REGION_TEXT", RAM);
REGION_ALIAS("REGION_RODATA", RAM);
REGION_ALIAS("REGION_DATA", RAM);
REGION_ALIAS("REGION_BSS", RAM);
REGION_ALIAS("REGION_HEAP", RAM);
REGION_ALIAS("REGION_STACK", RAM);

/* The K210 is dual-core and both harts leave the mask ROM. riscv-rt sends any
   hart above _max_hart_id straight to a busy-loop `abort`; naming the second
   one instead lets it reach _mp_hook, which parks it in `wfi`. Same outcome,
   less power, and the intent is visible.

   The heap here is a static array in .bss rather than the linker's .heap
   region, so _heap_size stays 0 and .stack gets everything between the end of
   .bss and the top of SRAM — over a megabyte. _hart_stack_size only positions
   the second hart's stack pointer below the first's. */
_max_hart_id = 1;
_hart_stack_size = 64K;

/* .eh_frame is emitted with 32-bit PC-relative relocations and placed at
   address 0, which cannot reach code at 0x80000000 — the link fails with
   "relocation R_RISCV_32_PCREL out of range" rather than producing anything.
   Nothing here unwinds, so discard it. */
SECTIONS
{
  /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) }
}
