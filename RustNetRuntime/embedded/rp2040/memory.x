/* RP2040: the ROM loads 256 bytes from flash offset 0, checks their CRC, and
   runs them. That stage sets up execute-in-place; the image proper starts
   immediately after it. Getting this layout wrong is a board that enumerates
   as BOOTSEL again on the next power-up and never runs anything. */
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

SECTIONS {
    .boot2 ORIGIN(BOOT2) : {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;
