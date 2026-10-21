"""Read-only PE audit for the bm2dx.dll hook constants in src/plugin.rs."""
from pathlib import Path
import hashlib
import re
import struct
import sys

root = Path(__file__).resolve().parents[1]
source = (root / "src/plugin.rs").read_text(encoding="utf-8")
path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(r"C:\IIDX\bm2dx.dll")
data = path.read_bytes()


def number(name):
    match = re.search(rf"const {name}: (?:u32|u64|usize) = (0x[0-9a-fA-F]+);", source)
    if not match:
        raise ValueError(name)
    return int(match.group(1), 16)


def string(name):
    match = re.search(rf'const {name}: &str = "([0-9a-f]+)";', source)
    if not match:
        raise ValueError(name)
    return match.group(1)


def u16(off): return struct.unpack_from("<H", data, off)[0]
def u32(off): return struct.unpack_from("<I", data, off)[0]
def u64(off): return struct.unpack_from("<Q", data, off)[0]


pe = u32(0x3c)
assert data[:2] == b"MZ" and data[pe:pe + 4] == b"PE\0\0"
section_count = u16(pe + 6)
optional_size = u16(pe + 20)
optional = pe + 24
assert u16(optional) == 0x20b
image_base = u64(optional + 24)
size_image = u32(optional + 56)
timestamp = u32(pe + 8)
sections = []
for i in range(section_count):
    off = optional + optional_size + i * 40
    name = data[off:off + 8].split(b"\0", 1)[0].decode("ascii")
    sections.append((name, u32(off + 12), u32(off + 8), u32(off + 20), u32(off + 16)))


def file_offset(rva):
    for name, virtual, vsize, raw, rawsize in sections:
        if virtual <= rva < virtual + max(vsize, rawsize):
            result = raw + rva - virtual
            if result < len(data):
                return result
    raise ValueError(f"unmapped RVA 0x{rva:x}")


def block(begin, end):
    start = file_offset(begin)
    stop = file_offset(end - 1) + 1
    return data[start:stop]


def fnv(content):
    result = 14695981039346656037
    for byte in content:
        result = ((result ^ byte) * 1099511628211) & ((1 << 64) - 1)
    return result


print(f"{path}: sha256={hashlib.sha256(data).hexdigest()}")
print(f"timestamp={timestamp:08x} expected={number('GAME_TIMESTAMP'):08x}; image={size_image:x} expected={number('GAME_IMAGE_SIZE'):x}")
failed = timestamp != number("GAME_TIMESTAMP") or size_image != number("GAME_IMAGE_SIZE")
for name in ("RANK", "PROFILE", "REG", "REG_RESPONSE", "APPOINT"):
    content = block(number(f"{name}_RVA"), number(f"{name}_END"))
    if name in ("PROFILE", "REG_RESPONSE"):
        prefix = hashlib.sha256(content[:32]).hexdigest()
        tail = hashlib.sha256(content[32:]).hexdigest()
        ok = prefix == string(f"{name}_PREFIX_SHA256") and tail == string(f"{name}_TAIL_SHA256")
    else:
        prefix, tail = fnv(content[:32]), fnv(content[32:])
        ok = prefix == number(f"{name}_PREFIX_HASH") and tail == number(f"{name}_TAIL_HASH")
    slot_rva = number("REG_SLOT") + 8 if name == "REG_RESPONSE" else number(f"{name}_SLOT")
    slot = u64(file_offset(slot_rva))
    target = image_base + number(f"{name}_RVA")
    slot_ok = slot == target
    print(f"{name}: code={'OK' if ok else 'MISMATCH'} slot={'OK' if slot_ok else f'{slot:x} != {target:x}'}")
    failed |= not ok or not slot_ok
sys.exit(1 if failed else 0)
