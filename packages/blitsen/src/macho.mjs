import { createHash } from "node:crypto";

// The narrow Mach-O writer used by the Phase 2 bundle linker (#256). Blitsen
// ships one thin 64-bit runtime per Darwin architecture, so fat files and
// 32-bit headers are deliberately rejected by the runtime resolver before this
// point. All integer fields in a modern Mach-O executable are little-endian.
const MH_MAGIC_64 = 0xfeedfacf;
const MH_EXECUTE = 2;
const CPU_TYPE_X86_64 = 0x01000007;
const CPU_TYPE_ARM64 = 0x0100000c;

const LC_SEGMENT_64 = 0x19;
const LC_SYMTAB = 0x2;
const LC_DYSYMTAB = 0xb;
const LC_TWOLEVEL_HINTS = 0x16;
const LC_CODE_SIGNATURE = 0x1d;
const LC_SEGMENT_SPLIT_INFO = 0x1e;
const LC_ENCRYPTION_INFO = 0x21;
const LC_DYLD_INFO = 0x22;
const LC_FUNCTION_STARTS = 0x26;
const LC_DATA_IN_CODE = 0x29;
const LC_DYLIB_CODE_SIGN_DRS = 0x2b;
const LC_ENCRYPTION_INFO_64 = 0x2c;
const LC_LINKER_OPTIMIZATION_HINT = 0x2d;
const LC_NOTE = 0x31;
const LC_DYLD_EXPORTS_TRIE = 0x80000033;
const LC_DYLD_CHAINED_FIXUPS = 0x80000034;
const LC_FILESET_ENTRY = 0x80000035;
const LC_ATOM_INFO = 0x36;
const LC_FUNCTION_VARIANTS = 0x37;
const LC_FUNCTION_VARIANT_FIXUPS = 0x38;
const LC_DYLD_INFO_ONLY = 0x80000022;

const HEADER_SIZE = 32;
const SEGMENT_COMMAND_SIZE = 72;
const SECTION_SIZE = 80;
// The shipped arm64 runtime has 96 bytes of load-command padding. A segment
// command fits at 72 bytes; adding an 80-byte section record would not. Keep
// the application in a named read-only segment and place its trailer at the
// segment end, which preserves an exact discoverable boundary without moving
// the executable's first mapped page.
const NEW_SEGMENT_COMMAND_SIZE = SEGMENT_COMMAND_SIZE;
const BUNDLE_TRAILER_SIZE = 64;
const LINKEDIT_DATA_COMMANDS = new Set([
  LC_SEGMENT_SPLIT_INFO, LC_FUNCTION_STARTS, LC_DATA_IN_CODE, LC_DYLIB_CODE_SIGN_DRS, LC_ATOM_INFO,
  LC_LINKER_OPTIMIZATION_HINT, LC_DYLD_EXPORTS_TRIE, LC_DYLD_CHAINED_FIXUPS,
  LC_FUNCTION_VARIANTS, LC_FUNCTION_VARIANT_FIXUPS,
]);

const CSMAGIC_CODEDIRECTORY = 0xfade0c02;
const CSMAGIC_EMBEDDED_SIGNATURE = 0xfade0cc0;
const CODE_DIRECTORY_SIZE = 88;
const SIGNATURE_IDENTIFIER = Buffer.from("blitsen\0");
const SIGN_PAGE_SIZE = 4096;

function align(value, boundary) {
  return Math.ceil(value / boundary) * boundary;
}

function nameAt(bytes, offset) {
  const field = bytes.subarray(offset, offset + 16);
  const end = field.indexOf(0);
  return field.subarray(0, end < 0 ? field.length : end).toString("ascii");
}

function fixedName(value) {
  const result = Buffer.alloc(16);
  result.write(value, 0, 16, "ascii");
  return result;
}

function malformed(reason) {
  throw new Error(`cannot link the application into the Mach-O runtime: ${reason}`);
}

function parseMachO(executable) {
  if (executable.length < 4 || executable.readUInt32LE(0) !== MH_MAGIC_64) return null;
  if (executable.length < HEADER_SIZE) malformed("the 64-bit header is truncated");
  if (executable.readUInt32LE(12) !== MH_EXECUTE) malformed("the input is not an executable");
  const cpu = executable.readInt32LE(4);
  if (cpu !== CPU_TYPE_X86_64 && cpu !== CPU_TYPE_ARM64)
    malformed(`CPU type 0x${(cpu >>> 0).toString(16)} is not a shipping Darwin architecture`);
  const commands = [];
  const ncmds = executable.readUInt32LE(16);
  const sizeofcmds = executable.readUInt32LE(20);
  const commandsEnd = HEADER_SIZE + sizeofcmds;
  if (commandsEnd > executable.length) malformed("the load-command table is truncated");
  let offset = HEADER_SIZE;
  for (let index = 0; index < ncmds; index += 1) {
    if (offset + 8 > commandsEnd) malformed(`load command ${index} has no complete header`);
    const type = executable.readUInt32LE(offset);
    const size = executable.readUInt32LE(offset + 4);
    if (size < 8 || size % 8 !== 0 || offset + size > commandsEnd)
      malformed(`load command ${index} has invalid size ${size}`);
    commands.push({ type, size, offset });
    offset += size;
  }
  if (offset !== commandsEnd) malformed("ncmds and sizeofcmds disagree");

  const segments = commands.filter(command => command.type === LC_SEGMENT_64).map(command => ({
    ...command,
    name: nameAt(executable, command.offset + 8),
    vmaddr: Number(executable.readBigUInt64LE(command.offset + 24)),
    vmsize: Number(executable.readBigUInt64LE(command.offset + 32)),
    fileoff: Number(executable.readBigUInt64LE(command.offset + 40)),
    filesize: Number(executable.readBigUInt64LE(command.offset + 48)),
    nsects: executable.readUInt32LE(command.offset + 64),
  }));
  const linkedit = segments.find(segment => segment.name === "__LINKEDIT");
  const text = segments.find(segment => segment.name === "__TEXT");
  if (!linkedit || !text) malformed("the __TEXT or __LINKEDIT segment is missing");
  if (segments.some(segment => segment.name === "__BLITSEN"))
    malformed("the runtime already contains a __BLITSEN segment");
  if (linkedit.fileoff + linkedit.filesize !== executable.length)
    malformed("__LINKEDIT is not the final bytes of the input runtime");

  const signatureCommand = commands.find(command => command.type === LC_CODE_SIGNATURE) ?? null;
  let signatureOffset = executable.length;
  let signatureSize = 0;
  if (signatureCommand) {
    if (signatureCommand.size !== 16) malformed("LC_CODE_SIGNATURE has an invalid size");
    signatureOffset = executable.readUInt32LE(signatureCommand.offset + 8);
    signatureSize = executable.readUInt32LE(signatureCommand.offset + 12);
    if (signatureOffset < linkedit.fileoff
      || signatureOffset + signatureSize !== executable.length)
      malformed("the inherited code signature is not the final __LINKEDIT object");
  }

  const extraCommands = NEW_SEGMENT_COMMAND_SIZE + (signatureCommand ? 0 : 16);
  const sectionOffsets = segments.flatMap(segment => {
    const offsets = [];
    for (let index = 0; index < segment.nsects; index += 1) {
      const section = segment.offset + SEGMENT_COMMAND_SIZE + index * SECTION_SIZE;
      const size = Number(executable.readBigUInt64LE(section + 40));
      const fileoff = executable.readUInt32LE(section + 48);
      if (size > 0 && fileoff > 0) offsets.push(fileoff);
    }
    return offsets;
  });
  const firstFileData = sectionOffsets.reduce(
    (minimum, fileoff) => Math.min(minimum, fileoff), linkedit.fileoff);
  if (commandsEnd + extraCommands > firstFileData)
    malformed(`the header has ${firstFileData - commandsEnd} bytes of load-command padding, `
      + `but embedding the payload needs ${extraCommands}`);
  if (!executable.subarray(commandsEnd, commandsEnd + extraCommands).every(byte => byte === 0))
    malformed("the load-command padding that would hold __BLITSEN is not empty");

  return {
    cpu, commands, linkedit, text, signatureCommand, signatureOffset, signatureSize,
    pageSize: cpu === CPU_TYPE_ARM64 ? 0x4000 : 0x1000,
  };
}

/** Returns the file offset at which a Mach-O payload section will be installed. */
export function machOPayloadOffset(executable) {
  const parsed = parseMachO(executable);
  return parsed?.linkedit.fileoff ?? null;
}

function shift32(command, at, amount, lower, upper) {
  const value = command.readUInt32LE(at);
  if (value !== 0 && value >= lower && value < upper) command.writeUInt32LE(value + amount, at);
}

function shift64(command, at, amount, lower, upper) {
  const value = Number(command.readBigUInt64LE(at));
  if (value !== 0 && value >= lower && value < upper)
    command.writeBigUInt64LE(BigInt(value + amount), at);
}

function patchCommand(command, type, amount, lower, upper, signatureOffset, signatureSize) {
  if (type === LC_SEGMENT_64) {
    const nsects = command.readUInt32LE(64);
    if (command.length < SEGMENT_COMMAND_SIZE + nsects * SECTION_SIZE)
      malformed("an LC_SEGMENT_64 command is shorter than its section table");
    for (let index = 0; index < nsects; index += 1) {
      const section = SEGMENT_COMMAND_SIZE + index * SECTION_SIZE;
      shift32(command, section + 48, amount, lower, upper);
      shift32(command, section + 56, amount, lower, upper);
    }
  } else if (type === LC_SYMTAB) {
    shift32(command, 8, amount, lower, upper);
    shift32(command, 16, amount, lower, upper);
  } else if (type === LC_DYSYMTAB) {
    for (const at of [32, 40, 48, 56, 64, 72]) shift32(command, at, amount, lower, upper);
  } else if (type === LC_DYLD_INFO || type === LC_DYLD_INFO_ONLY) {
    for (const at of [8, 16, 24, 32, 40]) shift32(command, at, amount, lower, upper);
  } else if (LINKEDIT_DATA_COMMANDS.has(type)) {
    shift32(command, 8, amount, lower, upper);
  } else if (type === LC_TWOLEVEL_HINTS
    || type === LC_ENCRYPTION_INFO || type === LC_ENCRYPTION_INFO_64) {
    shift32(command, 8, amount, lower, upper);
  } else if (type === LC_NOTE) {
    shift64(command, 24, amount, lower, upper);
  } else if (type === LC_FILESET_ENTRY) {
    shift64(command, 16, amount, lower, upper);
  }
  if (type === LC_CODE_SIGNATURE) {
    command.writeUInt32LE(signatureOffset, 8);
    command.writeUInt32LE(signatureSize, 12);
  }
}

function segmentCommand({ vmaddr, vmsize, fileoff, filesize }) {
  const command = Buffer.alloc(NEW_SEGMENT_COMMAND_SIZE);
  command.writeUInt32LE(LC_SEGMENT_64, 0);
  command.writeUInt32LE(NEW_SEGMENT_COMMAND_SIZE, 4);
  fixedName("__BLITSEN").copy(command, 8);
  command.writeBigUInt64LE(BigInt(vmaddr), 24);
  command.writeBigUInt64LE(BigInt(vmsize), 32);
  command.writeBigUInt64LE(BigInt(fileoff), 40);
  command.writeBigUInt64LE(BigInt(filesize), 48);
  command.writeUInt32LE(1, 56); // maxprot: VM_PROT_READ
  command.writeUInt32LE(1, 60); // initprot: VM_PROT_READ
  return command;
}

function signatureSize(codeLimit) {
  const slots = Math.ceil(codeLimit / SIGN_PAGE_SIZE);
  return 12 + 8 + CODE_DIRECTORY_SIZE + SIGNATURE_IDENTIFIER.length + slots * 32;
}

function adHocSignature(bytes, codeLimit, text) {
  const slots = Math.ceil(codeLimit / SIGN_PAGE_SIZE);
  const identOffset = CODE_DIRECTORY_SIZE;
  const hashOffset = identOffset + SIGNATURE_IDENTIFIER.length;
  const directorySize = hashOffset + slots * 32;
  const total = 12 + 8 + directorySize;
  const result = Buffer.alloc(total);
  result.writeUInt32BE(CSMAGIC_EMBEDDED_SIGNATURE, 0);
  result.writeUInt32BE(total, 4);
  result.writeUInt32BE(1, 8);
  result.writeUInt32BE(0, 12); // CSSLOT_CODEDIRECTORY
  result.writeUInt32BE(20, 16);
  result.writeUInt32BE(CSMAGIC_CODEDIRECTORY, 20);
  result.writeUInt32BE(directorySize, 24);
  result.writeUInt32BE(0x20400, 28);
  result.writeUInt32BE(0x20002, 32); // CS_ADHOC | CS_LINKER_SIGNED
  result.writeUInt32BE(hashOffset, 36);
  result.writeUInt32BE(identOffset, 40);
  result.writeUInt32BE(0, 44);
  result.writeUInt32BE(slots, 48);
  result.writeUInt32BE(codeLimit, 52);
  result[56] = 32;
  result[57] = 2; // SHA-256
  result[59] = 12; // 4 KiB code-signing pages
  result.writeBigUInt64BE(BigInt(text.fileoff), 84);
  result.writeBigUInt64BE(BigInt(text.filesize), 92);
  result.writeBigUInt64BE(1n, 100); // CS_EXECSEG_MAIN_BINARY
  SIGNATURE_IDENTIFIER.copy(result, 20 + identOffset);
  let hashAt = 20 + hashOffset;
  for (let offset = 0; offset < codeLimit; offset += SIGN_PAGE_SIZE) {
    createHash("sha256").update(bytes.subarray(offset, Math.min(offset + SIGN_PAGE_SIZE, codeLimit)))
      .digest().copy(result, hashAt);
    hashAt += 32;
  }
  return result;
}

/**
 * Installs `sectionData` in a read-only `__BLITSEN` segment immediately before
 * `__LINKEDIT`, then replaces the now-invalid inherited signature with a
 * deterministic ad-hoc signature. `codesign --force` may replace it later;
 * having one here keeps an ordinary unsigned arm64 export runnable too.
 */
export function injectMachOPayload(executable, sectionData) {
  const parsed = parseMachO(executable);
  if (!parsed) return null;
  const { commands, linkedit, text, signatureCommand,
    signatureOffset: oldSignatureOffset, pageSize } = parsed;
  if (sectionData.length < BUNDLE_TRAILER_SIZE) malformed("the bundle has no complete trailer");
  const segmentFilesize = align(sectionData.length, pageSize);
  const segmentVmsize = segmentFilesize;
  const linkeditData = executable.subarray(linkedit.fileoff, oldSignatureOffset);
  const newLinkeditOffset = linkedit.fileoff + segmentFilesize;
  const newSignatureOffset = newLinkeditOffset + linkeditData.length;
  const newSignatureSize = signatureSize(newSignatureOffset);
  const newLinkeditFilesize = linkeditData.length + newSignatureSize;
  const newLinkeditVmsize = align(Math.max(newLinkeditFilesize, pageSize), pageSize);

  const header = Buffer.from(executable.subarray(0, HEADER_SIZE));
  header.writeUInt32LE(header.readUInt32LE(16) + 1 + (signatureCommand ? 0 : 1), 16);
  header.writeUInt32LE(header.readUInt32LE(20) + NEW_SEGMENT_COMMAND_SIZE
    + (signatureCommand ? 0 : 16), 20);
  const embedded = segmentCommand({
    vmaddr: linkedit.vmaddr, vmsize: segmentVmsize,
    fileoff: linkedit.fileoff, filesize: segmentFilesize,
  });

  const rebuiltCommands = [];
  let inserted = false;
  for (const record of commands) {
    if (record.offset === linkedit.offset) {
      rebuiltCommands.push(embedded);
      if (!signatureCommand) {
        const codeSignature = Buffer.alloc(16);
        codeSignature.writeUInt32LE(LC_CODE_SIGNATURE, 0);
        codeSignature.writeUInt32LE(16, 4);
        codeSignature.writeUInt32LE(newSignatureOffset, 8);
        codeSignature.writeUInt32LE(newSignatureSize, 12);
        rebuiltCommands.push(codeSignature);
      }
      inserted = true;
    }
    const command = Buffer.from(executable.subarray(record.offset, record.offset + record.size));
    patchCommand(command, record.type, segmentFilesize, linkedit.fileoff,
      oldSignatureOffset, newSignatureOffset, newSignatureSize);
    if (record.offset === linkedit.offset) {
      command.writeBigUInt64LE(BigInt(linkedit.vmaddr + segmentVmsize), 24);
      command.writeBigUInt64LE(BigInt(newLinkeditVmsize), 32);
      command.writeBigUInt64LE(BigInt(newLinkeditOffset), 40);
      command.writeBigUInt64LE(BigInt(newLinkeditFilesize), 48);
    }
    rebuiltCommands.push(command);
  }
  if (!inserted) malformed("could not place __BLITSEN before __LINKEDIT");

  const commandBytes = Buffer.concat(rebuiltCommands);
  const newCommandsEnd = HEADER_SIZE + commandBytes.length;
  const bodyBeforeLinkedit = executable.subarray(newCommandsEnd, linkedit.fileoff);
  const trailerAt = sectionData.length - BUNDLE_TRAILER_SIZE;
  const unsigned = Buffer.concat([
    header,
    commandBytes,
    bodyBeforeLinkedit,
    sectionData.subarray(0, trailerAt),
    Buffer.alloc(segmentFilesize - sectionData.length),
    sectionData.subarray(trailerAt),
    linkeditData,
  ]);
  if (unsigned.length !== newSignatureOffset)
    malformed("the rebuilt segment offsets do not agree with the bytes written");
  const signature = adHocSignature(unsigned, newSignatureOffset, text);
  if (signature.length !== newSignatureSize)
    malformed("the ad-hoc signature size changed while it was being written");
  return Buffer.concat([unsigned, signature]);
}
