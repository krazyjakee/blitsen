// One synthetic signed Mach-O runtime shared by the JavaScript writer tests and
// the cross-language Rust↔JavaScript conformance gate. It deliberately carries
// just enough load-command structure to exercise segment insertion, offset
// shifting, and replacement of an inherited signature on both Darwin CPUs.
export function machoFixture(cpu) {
  const page = cpu === 0x0100000c ? 0x4000 : 0x1000;
  const linkeditAt = page;
  const linkeditBytes = 64;
  const inheritedSignatureBytes = 256;
  const signatureAt = linkeditAt + linkeditBytes;
  const commands = [];
  const segment = ({ name, vmaddr, vmsize, fileoff, filesize }) => {
    const command = Buffer.alloc(72);
    command.writeUInt32LE(0x19, 0);
    command.writeUInt32LE(72, 4);
    command.write(name, 8, 16, "ascii");
    command.writeBigUInt64LE(BigInt(vmaddr), 24);
    command.writeBigUInt64LE(BigInt(vmsize), 32);
    command.writeBigUInt64LE(BigInt(fileoff), 40);
    command.writeBigUInt64LE(BigInt(filesize), 48);
    command.writeUInt32LE(7, 56);
    command.writeUInt32LE(name === "__TEXT" ? 5 : 1, 60);
    return command;
  };
  commands.push(segment({
    name: "__TEXT", vmaddr: 0x100000000, vmsize: page,
    fileoff: 0, filesize: page,
  }));
  const symtab = Buffer.alloc(24);
  symtab.writeUInt32LE(0x2, 0);
  symtab.writeUInt32LE(24, 4);
  symtab.writeUInt32LE(linkeditAt, 8);
  symtab.writeUInt32LE(signatureAt - 32, 16);
  symtab.writeUInt32LE(32, 20);
  commands.push(symtab);
  const signature = Buffer.alloc(16);
  signature.writeUInt32LE(0x1d, 0);
  signature.writeUInt32LE(16, 4);
  signature.writeUInt32LE(signatureAt, 8);
  signature.writeUInt32LE(inheritedSignatureBytes, 12);
  commands.push(signature);
  commands.push(segment({
    name: "__LINKEDIT", vmaddr: 0x100000000 + page, vmsize: page,
    fileoff: linkeditAt, filesize: linkeditBytes + inheritedSignatureBytes,
  }));
  const commandBytes = Buffer.concat(commands);
  const executable = Buffer.alloc(signatureAt + inheritedSignatureBytes);
  executable.writeUInt32LE(0xfeedfacf, 0);
  executable.writeInt32LE(cpu, 4);
  executable.writeUInt32LE(3, 8);
  executable.writeUInt32LE(2, 12);
  executable.writeUInt32LE(commands.length, 16);
  executable.writeUInt32LE(commandBytes.length, 20);
  executable.writeUInt32LE(0x200085, 24);
  commandBytes.copy(executable, 32);
  executable.fill(0x5a, linkeditAt, signatureAt);
  executable.fill(0xa5, signatureAt);
  return { executable, linkeditAt, page };
}
