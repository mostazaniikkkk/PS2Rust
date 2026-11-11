/* Linker script para ejecutables de PlayStation 2 (Emotion Engine).
 * La memoria de usuario empieza en 0x00100000; el kernel vive debajo. */
ENTRY(_start)

__stack_size = DEFINED(__stack_size) ? __stack_size : 0x40000; /* 256 KiB */

PHDRS
{
  text PT_LOAD FLAGS(7); /* RWX: un solo segmento, como los ELF comerciales */
}

SECTIONS
{
  . = 0x00100000;

  .text : ALIGN(16)
  {
    KEEP(*(.text.start))
    *(.text .text.*)
  } :text

  .rodata : ALIGN(16)
  {
    *(.rodata .rodata.*)
    *(.lit4 .lit8)
  } :text

  .data : ALIGN(16)
  {
    *(.data .data.*)
    *(.sdata .sdata.*)
  } :text

  .bss (NOLOAD) : ALIGN(16)
  {
    __bss_start = .;
    *(.sbss .sbss.*)
    *(.scommon)
    *(.bss .bss.*)
    *(COMMON)
    . = ALIGN(16);
    __bss_end = .;
  } :text

  . = ALIGN(64);
  _end = .;

  /DISCARD/ :
  {
    *(.reginfo)
    *(.MIPS.abiflags)
    *(.MIPS.options)
    *(.pdr)
    *(.eh_frame .eh_frame.*)
    *(.gnu.attributes)
    *(.comment)
    *(.note .note.*)
  }
}
