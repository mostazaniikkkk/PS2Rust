/* Linker script para módulos IRX del IOP (el procesador de E/S de la PS2).
 *
 * Se enlaza en la dirección 0 conservando las relocaciones (--emit-relocs);
 * cargo-ps2 convierte después el resultado al formato IRX, que el cargador
 * de la BIOS reubica donde haya memoria libre.
 *
 * El cargador busca las tablas de importación sólo dentro de .text, así que
 * las constantes (.rodata) van con los datos. */
ENTRY(_start)

SECTIONS
{
  . = 0;

  .text : ALIGN(16)
  {
    LONG(0) LONG(0) LONG(0) LONG(0)   /* nada útil en la dirección 0 */
    KEEP(*(.text.start))
    *(.text .text.*)
    . = ALIGN(16);
  } =0x00000000

  .data : ALIGN(16)
  {
    *(.rodata .rodata.*)
    *(.lit4 .lit8)
    . = ALIGN(16);
    KEEP(*(.data.irx_id))
    *(.data .data.*)
    *(.sdata .sdata.*)
    *(.got .got.*)
    . = ALIGN(16);
  }

  .bss (NOLOAD) : ALIGN(16)
  {
    *(.sbss .sbss.*)
    *(.scommon)
    *(.bss .bss.*)
    *(COMMON)
    . = ALIGN(16);
  }

  /* Detrás de todo: aquí añade cargo-ps2 los trampolines de la corrección de
   * HI/LO de MIPS I (ver mips1.rs) y las palabras auxiliares de las
   * relocaciones (ver irx.rs). Sólo ocupan lo que haga falta. */
  __irxtramp_start = ALIGN(16);

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
