#!/usr/bin/env python3

import sys
import os
import subprocess
import argparse
import re
from collections import defaultdict
import math

# --- Configuration ---
TOOLCHAIN_PREFIX = "arm-none-eabi-"
DEFAULT_MEMORY_REGIONS = {
    # RAM (xrw)       : ORIGIN = 0x20000004, LENGTH = 20K - 4 = 20476
    "RAM":        (0x20000000, 20480),
    "FLASH":      (0x08000000, 65536),
}
SIZE_TOOL = f"{TOOLCHAIN_PREFIX}size"
# --- End Configuration ---

def format_bytes(size_bytes):
    """Formats bytes into KB, MB, GB"""
    if size_bytes < 0: return f"({abs(size_bytes)} B)"
    if size_bytes == 0: return "0 B"
    power = math.floor(math.log(max(1, size_bytes), 1024))
    if power == 0: return f"{size_bytes} B"
    elif power == 1: return f"{size_bytes / 1024:.2f} KB"
    elif power == 2: return f"{size_bytes / (1024**2):.2f} MB"
    else: return f"{size_bytes / (1024**3):.2f} GB"

def parse_linker_size(size_str):
    """Parses linker script size strings like '20K', '0x1000', '64K - 4K'."""
    size_str = size_str.strip()
    if '-' in size_str:
        parts = [p.strip() for p in size_str.split('-', 1)]
        if len(parts) == 2:
            try:
                val1 = parse_linker_size(parts[0])
                val2 = parse_linker_size(parts[1])
                return val1 - val2
            except ValueError as e:
                raise ValueError(f"Cannot parse subtraction expression '{size_str}': {e}") from e
        else: raise ValueError(f"Invalid subtraction format: {size_str}")
    multiplier = 1
    if size_str.upper().endswith('K'):
        multiplier = 1024
        size_str = size_str[:-1].strip()
    elif size_str.upper().endswith('M'):
        multiplier = 1024 * 1024
        size_str = size_str[:-1].strip()
    try: return int(size_str, 0) * multiplier
    except ValueError: raise ValueError(f"Invalid size value format: {size_str}")

def parse_linker_memory_file(filepath):
    """Parses a linker memory definition file to extract MEMORY regions and aliases."""
    print(f"Parsing linker memory file: {filepath}")
    regions = {}
    aliases_to_resolve = []
    try:
        with open(filepath, 'r', encoding='utf-8') as f: content = f.read()
        content = re.sub(r'/\*.*?\*/', '', content, flags=re.DOTALL)
        content = re.sub(r'//.*?$', '', content, flags=re.MULTILINE)
        memory_match = re.search(r'MEMORY\s*\{([^}]+)\}', content, re.IGNORECASE | re.DOTALL)
        if not memory_match:
            print(f"Warning: Could not find MEMORY {{...}} block in {filepath}", file=sys.stderr); return None
        memory_block = memory_match.group(1)
        region_regex = re.compile(r"^\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\([^)]*\)\s*:\s*ORIGIN\s*=\s*([^,]+?)\s*,\s*LENGTH\s*=\s*([^,\s;}]+)", re.IGNORECASE | re.MULTILINE)
        for match in region_regex.finditer(memory_block):
            name, origin_str, length_str = match.groups(); name = name.upper()
            try:
                origin = parse_linker_size(origin_str); length = parse_linker_size(length_str)
                regions[name] = (origin, length)
                print(f"  Found Region: {name:<12} ORIGIN=0x{origin:08x}, LENGTH={length} ({format_bytes(length)})")
            except ValueError as e: print(f"  Warning: Skipping region '{name}'. Cannot parse values: {e}", file=sys.stderr)
        alias_regex = re.compile(r"^\s*REGION_ALIAS\s*\(\s*\"?([a-zA-Z_][a-zA-Z0-9_]*)\"?\s*,\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\)", re.IGNORECASE | re.MULTILINE)
        for match in alias_regex.finditer(content):
            alias_name, target_name = match.groups(); alias_name = alias_name.upper(); target_name = target_name.upper()
            aliases_to_resolve.append((alias_name, target_name)); print(f"  Found Alias: {alias_name} -> {target_name}")
        for alias_name, target_name in aliases_to_resolve:
            if target_name in regions:
                if alias_name not in regions: regions[alias_name] = regions[target_name]; print(f"  Resolved Alias: {alias_name} using {target_name}'s definition")
                else: print(f"  Warning: Alias '{alias_name}' already defined, ignoring alias to '{target_name}'.", file=sys.stderr)
            else: print(f"  Warning: Cannot resolve alias '{alias_name}', target region '{target_name}' not found.", file=sys.stderr)
        if not regions: print(f"Warning: No valid memory regions parsed from {filepath}", file=sys.stderr); return None
        return regions
    except FileNotFoundError: print(f"Error: Linker memory file not found: {filepath}", file=sys.stderr); return None
    except Exception as e: print(f"Error parsing linker memory file {filepath}: {e}", file=sys.stderr); return None

def parse_size_output(size_output_lines):
    """Parses the output of 'arm-none-eabi-size -Ax'."""
    if not size_output_lines: print("Error: No input received from size tool", file=sys.stderr); return None
    sections = []; headers = []; total_val_hex = None; elf_basename = "unknown"
    try:
        first_line = size_output_lines[0].strip(); match = re.match(r'^(.*?)\s*:\s*$', first_line)
        if match: elf_basename = os.path.basename(match.group(1))
        else: print(f"Warning: Could not parse filename line: {first_line}", file=sys.stderr)
        if len(size_output_lines) > 1:
            headers = size_output_lines[1].split()
            if len(headers) != 3: print(f"Warning: Unexpected header format: {size_output_lines[1].strip()}", file=sys.stderr); headers = ["section", "size", "addr"]
        else: headers = ["section", "size", "addr"]
        for line in size_output_lines[2:]:
            line = line.strip();
            if not line: continue
            if line.startswith("Total"):
                parts = line.split();
                if len(parts) == 2: total_val_hex = parts[1]
                else: print(f"Warning: Unexpected Total line format: {line}", file=sys.stderr)
                continue
            parts = line.split(None, 2)
            if len(parts) == 3:
                name, size_hex, addr_hex = parts
                try: sections.append({'name': name, 'size': int(size_hex, 16), 'addr': int(addr_hex, 16)})
                except ValueError: print(f"Warning: Could not parse line values: {line}", file=sys.stderr)
            else:
                 if len(line.split()) == 1 and line.startswith('.'): sections.append({'name': line, 'size': 0, 'addr': 0}); # print(f"Warning: Assuming size 0 / addr 0 for line: {line}", file=sys.stderr) # Less verbose
                 else: print(f"Warning: Skipping malformed line: {line}", file=sys.stderr)
        total_size_dec = 0
        if total_val_hex:
            try: total_size_dec = int(total_val_hex, 16)
            except ValueError: print(f"Warning: Could not parse total size '{total_val_hex}'", file=sys.stderr); total_val_hex = None
        return elf_basename, headers, sections, total_size_dec
    except Exception as e: print(f"Error parsing size output: {e}", file=sys.stderr); return None

def print_cargo_style(elf_basename, headers, sections, total_size_dec):
    """Prints the size info in a format similar to cargo size."""
    print(f"{elf_basename}  :")
    max_name_len = len(headers[0]) if headers else len("section"); max_size_len = len(headers[1]) if headers else len("size"); max_addr_len = len(headers[2]) if headers else len("addr")
    for sec in sections:
        max_name_len = max(max_name_len, len(sec['name'])); max_size_len = max(max_size_len, len(str(sec['size']))); max_addr_len = max(max_addr_len, len(f"0x{sec['addr']:x}"))
    max_name_len = max(max_name_len, len("Total"))
    if total_size_dec is not None: max_size_len = max(max_size_len, len(str(total_size_dec)))
    header_names = headers if headers else ["section", "size", "addr"]
    print(f"{header_names[0]:<{max_name_len}} {header_names[1]:>{max_size_len}} {header_names[2]:>{max_addr_len}}")
    for sec in sections: addr_str = f"0x{sec['addr']:x}"; print(f"{sec['name']:<{max_name_len}} {sec['size']:>{max_size_len}} {addr_str:>{max_addr_len}}")
    if total_size_dec is not None: addr_padding = ' ' * max_addr_len; print(f"{'Total':<{max_name_len}} {total_size_dec:>{max_size_len}} {addr_padding}")

def print_specific_sum(sections):
    """Calculates and prints the sum of vector table, .text, and .rodata."""
    sum_sections = {'.vectors', '.vector_table', '.text', '.rodata'}
    total = 0
    found_sections = []
    for sec in sections:
        if sec['name'] in sum_sections:
            display_name = '.vector_table' if sec['name'] in {'.vectors', '.vector_table'} else sec['name']
            if display_name not in found_sections:
                 found_sections.append(display_name)
            total += sec['size']
    if found_sections:
        print(f"\nSum of {' + '.join(sorted(list(found_sections)))}: {total} bytes")
    else:
        print("\nWarning: Did not find vector table, .text, or .rodata sections for summation.")

def print_memory_region_summary(sections, memory_regions):
    """Prints a filtered CMake-like memory region usage summary with sizes in bytes."""
    print("\nMemory Region Summary:")
    if not memory_regions: print("  No memory regions defined or parsed."); return
    regions_to_display = ["FLASH", "RAM"]; display_data = {}

    # --- Calculate FLASH Usage (Code + ROData + Initialized Data LMA + Vector Table) ---
    flash_region_name = "FLASH"
    if flash_region_name in memory_regions:
        flash_region_start, flash_region_size = memory_regions[flash_region_name]
        flash_sections = {'.vectors', '.vector_table', '.text', '.rodata', '.ARM.exidx', '.ARM.extab', '.init_array', '.fini_array', '.glue_7', '.glue_7t', '.startup', '.data'}
        flash_used_size = sum(sec['size'] for sec in sections if sec['name'] in flash_sections)
        display_data[flash_region_name] = {"used": flash_used_size, "total": flash_region_size}

    # --- Calculate RAM Usage (Span including gaps for .data, .bss, .heap, .stack, .uninit) ---
    ram_region_name = "RAM"
    if ram_region_name in memory_regions:
        ram_region_start, ram_region_size = memory_regions[ram_region_name]; ram_used_size = 0
        ram_sections = []
        for sec in sections:
            sec_start = sec['addr']
            if ram_region_start <= sec_start < (ram_region_start + ram_region_size):
                 # Include sections typically residing in RAM (VMA in RAM)
                 if sec['name'] in {'.data', '.bss', '.heap', '.stack_dummy', '.uninit'} or 'ram' in sec['name'].lower():
                    ram_sections.append(sec)
        if not ram_sections: ram_used_size = 0
        else:
            ram_sections.sort(key=lambda s: s['addr']); sum_of_sizes = sum(s['size'] for s in ram_sections)
            first_section_start = ram_sections[0]['addr']; gap_before_first = max(0, first_section_start - ram_region_start)
            inter_section_gaps = 0
            for i in range(len(ram_sections) - 1):
                current_end = ram_sections[i]['addr'] + ram_sections[i]['size']; next_start = ram_sections[i+1]['addr']
                gap = max(0, next_start - current_end); inter_section_gaps += gap
            ram_used_size = sum_of_sizes + gap_before_first + inter_section_gaps
            if ram_used_size > ram_region_size: print(f"  Warning: Calculated RAM usage ({ram_used_size}) exceeds region size ({ram_region_size}). ", file=sys.stderr)
        display_data[ram_region_name] = {"used": ram_used_size, "total": ram_region_size}

    # --- Calculate Column Widths & Print ---
    max_name_len = len("Memory region"); max_used_len = len("Used Size"); max_total_len = len("Region Size"); max_perc_len = len("%age Used")
    for name in regions_to_display:
        if name in display_data:
            max_name_len = max(max_name_len, len(name)); max_used_len = max(max_used_len, len(str(display_data[name]['used']))); max_total_len = max(max_total_len, len(str(display_data[name]['total'])))
            total = display_data[name]['total']; used = display_data[name]['used']
            if total > 0: max_perc_len = max(max_perc_len, len(f"{used * 100.0 / total:.2f}%"))
            else: max_perc_len = max(max_perc_len, len("N/A"))
    print(f"  {'Memory region':<{max_name_len}} {'Used Size':>{max_used_len}} {'Region Size':>{max_total_len}} {'%age Used':>{max_perc_len}}")
    for name in regions_to_display:
         if name in display_data:
            data = display_data[name]; used = data['used']; total = data['total']
            used_str = str(used); total_str = str(total)
            if total > 0: perc_str = f"{used * 100.0 / total:.2f}%"
            else: perc_str = "N/A" if total == 0 else "inf%" if used > 0 else "0.00%"
            print(f"  {name:<{max_name_len}} {used_str:>{max_used_len}} {total_str:>{max_total_len}} {perc_str:>{max_perc_len}}")

# REMOVED print_final_summary function

def main():
    parser = argparse.ArgumentParser(description="Displays ELF size info & summaries.",formatter_class=argparse.RawTextHelpFormatter)
    parser.add_argument("elf_file", help="Path to the ELF file.")
    parser.add_argument("--linker-memory", dest="linker_memory_file", help="Path to linker memory definition file.")
    args = parser.parse_args()

    if not os.path.exists(args.elf_file): print(f"Error: File not found: {args.elf_file}", file=sys.stderr); sys.exit(1)

    active_memory_regions = None
    if args.linker_memory_file:
        parsed_regions = parse_linker_memory_file(args.linker_memory_file)
        if parsed_regions: active_memory_regions = parsed_regions
        else: print("Warning: Failed parse linker memory file. Falling back to defaults.", file=sys.stderr); active_memory_regions = DEFAULT_MEMORY_REGIONS
    else: print("Info: Using internal default memory regions."); active_memory_regions = DEFAULT_MEMORY_REGIONS

    cmd = [SIZE_TOOL, "-Ax", args.elf_file]; print(f"\nRunning: {' '.join(cmd)}")
    try: result = subprocess.run(cmd, capture_output=True, text=True, check=False, encoding='utf-8', errors='ignore')
    except FileNotFoundError: print(f"\nError: Command not found: '{SIZE_TOOL}'. Is toolchain in PATH?", file=sys.stderr); sys.exit(1)
    except Exception as e: print(f"\nError running {SIZE_TOOL}: {e}", file=sys.stderr); sys.exit(1)

    if result.returncode != 0: print(f"\nError: {SIZE_TOOL} failed ({result.returncode})\n--- stderr ---\n{result.stderr}\n--------------", file=sys.stderr); sys.exit(1)

    size_lines = result.stdout.splitlines(); parsed_data = parse_size_output(size_lines)
    if parsed_data:
        elf_basename, headers, sections, total_size_dec = parsed_data
        print("-" * 60); print_cargo_style(elf_basename, headers, sections, total_size_dec)
        print("-" * 60); print_specific_sum(sections)
        print("-" * 60); print_memory_region_summary(sections, active_memory_regions)
        # REMOVED call to print_final_summary
        print("-" * 60) # Keep final separator for neatness
    else: print("\nError: Failed to parse the output from the size tool.", file=sys.stderr); sys.exit(1)

if __name__ == "__main__":
    main()
