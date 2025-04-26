/* memory.x - Memory configuration for STM32L071C8Tx with custom bootloader */

/* This file is derived from the C linker script LinkerScript.ld */
/* Device: STM32L071C8Tx */
/* Total Memory: 64K FLASH, 20K RAM */
/* Bootloader: Occupies first 4K of FLASH (0x08000000 - 0x08000FFF) */
/* Metadata: Occupies 256 bytes after bootloader (0x08001000 - 0x080010FF) */
/* Application: Starts after Bootloader and Metadata */

MEMORY
{
  /* RAM: Same as in C script */
  /* Starts at 0x20000000, 20 KiB length */
  RAM (rwx) : ORIGIN = 0x20000000, LENGTH = 20K

  /* FLASH: Available flash memory for the application */
  /* Starts after the 4K Bootloader and 256 byte Metadata sections */
  /* Origin = 0x08000000 + 4K + 256 = 0x08000000 + 0x1000 + 0x100 = 0x08001100 */
  /* Length = Total Flash - Bootloader - Metadata = 64K - 4K - 256 = 61184 bytes */
    FLASH : ORIGIN = 0x08001100, LENGTH = 60160 /* 64K - 4K - 256 bytes - 1K Storage */
}
