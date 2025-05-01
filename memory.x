MEMORY
{
  RAM (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
  /* -1 KB for storing user data */
  FLASH : ORIGIN = 0x08000000, LENGTH = 63K
}

