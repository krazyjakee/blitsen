import { open } from "node:fs/promises";

const ELF_MACHINES = { 0x03: "ia32", 0x28: "arm", 0x3e: "x64", 0xb7: "arm64" };
const MACHO_CPUS = { 0x00000007: "ia32", 0x0000000c: "arm", 0x01000007: "x64", 0x0100000c: "arm64" };
const PE_MACHINES = { 0x014c: "ia32", 0x01c4: "arm", 0x8664: "x64", 0xaa64: "arm64" };
const architecture = (table, code) => table[code] ?? `0x${code.toString(16)}`;

// Only the container header is read, and only far enough to name the machine a
// binary was built for. `kind` decides which containers count: a `.node` must be
// a dynamic object, and the Phase 2 runtime must be an executable — so a text
// file renamed `.node`, and an addon handed to the linker where the runtime
// belongs, are both rejected rather than described.
//
// ELF is the one format that cannot tell the two apart: a position-independent
// executable is `ET_DYN` exactly as a shared library is, and every runtime this
// project builds is one. So an executable accepts both types there, and Mach-O
// and PE, which do name the difference, are held to it.
function describeContainer(bytes, kind) {
  if (bytes.byteLength < 64) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, false) === 0x7f454c46) {
    const little = bytes[5] === 1;
    const type = view.getUint16(16, little);
    const acceptable = kind === "library" ? type === 3 : type === 2 || type === 3; // ET_EXEC, ET_DYN
    if (!acceptable) return null;
    return {
      format: "ELF",
      platform: "linux",
      architectures: [architecture(ELF_MACHINES, view.getUint16(18, little))],
    };
  }
  const magic = view.getUint32(0, true);
  if (magic === 0xfeedfacf || magic === 0xfeedface) {
    const filetype = view.getUint32(12, true);
    const acceptable = kind === "library"
      ? filetype === 6 || filetype === 8 // MH_DYLIB, MH_BUNDLE
      : filetype === 2; // MH_EXECUTE
    if (!acceptable) return null;
    return {
      format: "Mach-O",
      platform: "darwin",
      architectures: [architecture(MACHO_CPUS, view.getUint32(4, true))],
    };
  }
  // A universal binary carries one slice per architecture; any of them matching
  // the host is enough, because dyld picks the slice.
  if (view.getUint32(0, false) === 0xcafebabe) {
    const slices = view.getUint32(4, false);
    if (slices === 0 || bytes.byteLength < 8 + slices * 20) return null;
    return {
      format: "Mach-O universal",
      platform: "darwin",
      architectures: Array.from({ length: slices }, (_, index) =>
        architecture(MACHO_CPUS, view.getUint32(8 + index * 20, false))),
    };
  }
  if (bytes[0] === 0x4d && bytes[1] === 0x5a) {
    const header = view.getUint32(0x3c, true);
    if (bytes.byteLength < header + 24) return null;
    if (view.getUint32(header, true) !== 0x00004550) return null; // "PE\0\0"
    const library = (view.getUint16(header + 22, true) & 0x2000) !== 0; // IMAGE_FILE_DLL
    if (library !== (kind === "library")) return null;
    return {
      format: "PE",
      platform: "win32",
      architectures: [architecture(PE_MACHINES, view.getUint16(header + 4, true))],
    };
  }
  return null;
}

/** Names the platform a `.node` shared library was built for, or `null`. */
export function describeNativeBinary(bytes) {
  return describeContainer(bytes, "library");
}

/** Names the platform an executable was built for, or `null`. */
export function describeExecutableBinary(bytes) {
  return describeContainer(bytes, "executable");
}

// Enough of a file to read its container header, rather than the whole of it:
// the Phase 2 runtime is tens of megabytes and every byte past the PE header is
// noise for this question.
export async function readContainerHeader(path) {
  const handle = await open(path, "r");
  try {
    const buffer = Buffer.alloc(8192);
    const { bytesRead } = await handle.read(buffer, 0, buffer.byteLength, 0);
    return buffer.subarray(0, bytesRead);
  } finally {
    await handle.close();
  }
}
