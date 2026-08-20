/* STM32F401RET6 — Nucleo-F401RE.

   96 KB of RAM, in one piece at 0x20000000. There is no second bank and no
   CCM on this part, so the layout is the obvious one and there is nothing to
   be clever about.

   That 96 KB is also the reason this board reads assemblies but does not run
   them. Loading the runtime costs 192,045 bytes with the smallest useful set
   of RustBCL bindings — twice what the part has, before `.bss` and the stack
   take their share. The firmware reports that in a line of text rather than
   discovering it as an allocation failure; see `Tier::for_budget` in
   ../demo-common.

   FLASH is the full 512 KB. Nothing here writes to flash, so unlike the
   RustNet firmware this was modelled on there is no storage sector to keep
   the linker out of. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 512K
  RAM   : ORIGIN = 0x20000000, LENGTH = 96K
}
