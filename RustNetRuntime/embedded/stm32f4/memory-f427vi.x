/* STM32F427VIT6 — Netduino 3 WiFi.

   The part advertises 256 KB of RAM, and it is in two pieces that are not
   adjacent:

     - 192 KB at 0x20000000 (SRAM1 112K + SRAM2 16K + SRAM3 64K, contiguous)
     - 64 KB of CCM at 0x10000000, reachable by the core but not by DMA

   Handing only the first to the allocator leaves 192 KB, minus whatever
   `.data`, `.bss` and the stack take out of it — and the runtime needs
   192,045 bytes to load with the smallest useful set of RustBCL bindings.
   A few kilobytes of statics is the difference between this board running a
   C# program and not running one.

   So the two memories swap the roles you would expect:

     RAM  -> CCM.  `.data`, `.bss` and the stack live here. Nothing in this
             firmware uses DMA, which is the only thing CCM cannot do, and the
             core reaches it with no wait states.
     SRAM -> the managed heap, in one unbroken 192 KB piece.

   `cortex-m-rt`'s `link.x` hardcodes `> RAM` for `.data`, `.bss` and the
   stack, so naming CCM `RAM` is what moves them; it is not a typo.

   The heap gets its own output section rather than being a `static` in `.bss`,
   because a `static` in `.bss` would follow `.bss` into CCM. `(NOLOAD)` keeps
   192 KB of zeros out of the image and out of the startup memset — the
   allocator does not need its arena zeroed.

   The 4 KB reserve below `_ssram_heap` is the honest kind of margin: if a
   future change puts something else in SRAM, the linker fails rather than
   letting it overlap the heap.

   FLASH is the full 2 MB. Nothing here writes to flash. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 2M
  RAM   : ORIGIN = 0x10000000, LENGTH = 64K
  SRAM  : ORIGIN = 0x20000000, LENGTH = 192K
}

SECTIONS
{
  .sram_heap (NOLOAD) : ALIGN(8)
  {
    _ssram_heap = .;
    *(.sram_heap .sram_heap.*);
    . = ALIGN(8);
    _esram_heap = .;
  } > SRAM
} INSERT AFTER .bss;
