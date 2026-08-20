/* STM32F777 — Wilderness Labs Meadow F7 Micro v1.0.

   RAM starts at SRAM1 rather than at 0x20000000. The 128 KB below it is DTCM:
   the fastest memory on the part, but tightly coupled to the core and not
   reachable by DMA. Handing it to the allocator would work until the first
   driver DMAs into a buffer that happened to land there.

   So RAM is SRAM1 (368K) + SRAM2 (16K) = 384K contiguous at 0x20020000.

   FLASH stops at 1 MB of the part's 2 MB, leaving the upper megabyte free and
   ensuring the linker can never place code where an erase would land.

   This layout is taken from the RustNet Meadow F7 port in
   ../../../RustNet/runtime/firmware-meadow-f7, which established it. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 1M
  RAM   : ORIGIN = 0x20020000, LENGTH = 384K
}
