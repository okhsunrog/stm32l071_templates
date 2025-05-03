/* memory.x */
/* Linker memory layout definition for STM32L071xx (e.g., 64K Flash, 20K RAM) */
/* This file ONLY defines memory regions. */
/* Standard sections (.vector_table, .text, .data, .bss, stack, heap) are placed */
/* by the default linker script provided by cortex-m-rt / flip-link. */

MEMORY
{
  /* RAM */
  /* Adjust LENGTH based on your specific STM32L071 variant */
  RAM (rwx) : ORIGIN = 0x20000000, LENGTH = 20K

  /* Main firmware area */
  /* Sections like .vector_table, .text, .rodata will be placed here by the default script */
  /* LENGTH = Total Physical Flash - Storage Length */
  /* Adjust ORIGIN/LENGTH based on your specific STM32L071 variant */
  FLASH (rx) : ORIGIN = 0x08000000, LENGTH = 63K

  /* Storage area: 1KB at the end of physical Flash */
  /* This region is manually managed by storage.rs */
  /* Ensure ORIGIN = FLASH ORIGIN + FLASH LENGTH */
  STORAGE (rx) : ORIGIN = 0x08000000 + 63K, LENGTH = 1K
}

/* Define symbols for the custom storage region usable by Rust code */
/* These symbols MUST be defined here as they are specific to our application's storage module */
PROVIDE(__storage_start = ORIGIN(STORAGE));
PROVIDE(__storage_end = ORIGIN(STORAGE) + LENGTH(STORAGE));
