//! The Mach-O container half of the Phase 2 bundle writer (#256).

use sha2::{Digest, Sha256};

use super::{BundleError, TRAILER_SIZE, malformed};

const MH_MAGIC_64: u32 = 0xfeedfacf;
const MH_EXECUTE: u32 = 2;
const CPU_TYPE_X86_64: i32 = 0x01000007;
const CPU_TYPE_ARM64: i32 = 0x0100000c;

const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const LC_DYSYMTAB: u32 = 0xb;
const LC_TWOLEVEL_HINTS: u32 = 0x16;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const LC_SEGMENT_SPLIT_INFO: u32 = 0x1e;
const LC_ENCRYPTION_INFO: u32 = 0x21;
const LC_DYLD_INFO: u32 = 0x22;
const LC_FUNCTION_STARTS: u32 = 0x26;
const LC_DATA_IN_CODE: u32 = 0x29;
const LC_DYLIB_CODE_SIGN_DRS: u32 = 0x2b;
const LC_ENCRYPTION_INFO_64: u32 = 0x2c;
const LC_LINKER_OPTIMIZATION_HINT: u32 = 0x2d;
const LC_NOTE: u32 = 0x31;
const LC_DYLD_EXPORTS_TRIE: u32 = 0x80000033;
const LC_DYLD_CHAINED_FIXUPS: u32 = 0x80000034;
const LC_FILESET_ENTRY: u32 = 0x80000035;
const LC_ATOM_INFO: u32 = 0x36;
const LC_FUNCTION_VARIANTS: u32 = 0x37;
const LC_FUNCTION_VARIANT_FIXUPS: u32 = 0x38;
const LC_DYLD_INFO_ONLY: u32 = 0x80000022;

const HEADER_SIZE: usize = 32;
const SEGMENT_COMMAND_SIZE: usize = 72;
const SECTION_SIZE: usize = 80;
// A shipped arm64 runtime has 96 bytes of load-command padding. A segment
// command is 72 bytes; adding a section record would make it 152 and require
// moving the executable's first mapped page. The payload therefore occupies a
// named read-only segment directly. Its trailer is placed at the segment end,
// so the command still records the exact searchable container boundary.
const NEW_SEGMENT_COMMAND_SIZE: usize = SEGMENT_COMMAND_SIZE;

const CSMAGIC_CODEDIRECTORY: u32 = 0xfade0c02;
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade0cc0;
const CODE_DIRECTORY_SIZE: usize = 88;
const SIGNATURE_IDENTIFIER: &[u8] = b"blitsen\0";
const SIGN_PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy)]
struct Command {
    kind: u32,
    size: usize,
    offset: usize,
}

#[derive(Clone, Copy)]
struct Segment {
    command: Command,
    vmaddr: u64,
    fileoff: u64,
    filesize: u64,
    nsects: usize,
}

struct MachO {
    commands: Vec<Command>,
    linkedit: Segment,
    text: Segment,
    blitsen: Option<Segment>,
    signature: Option<Command>,
    signature_offset: usize,
    page_size: usize,
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, BundleError> {
    let value = bytes
        .get(at..at + 4)
        .ok_or_else(|| malformed("Mach-O integer is truncated"))?;
    Ok(u32::from_le_bytes(value.try_into().expect("four bytes")))
}

fn read_i32(bytes: &[u8], at: usize) -> Result<i32, BundleError> {
    Ok(read_u32(bytes, at)? as i32)
}

fn read_u64(bytes: &[u8], at: usize) -> Result<u64, BundleError> {
    let value = bytes
        .get(at..at + 8)
        .ok_or_else(|| malformed("Mach-O integer is truncated"))?;
    Ok(u64::from_le_bytes(value.try_into().expect("eight bytes")))
}

fn write_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn write_be_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

fn write_be_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_be_bytes());
}

fn fixed_name(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + 16].fill(0);
    bytes[at..at + value.len()].copy_from_slice(value);
}

fn name(bytes: &[u8], at: usize) -> Result<&[u8], BundleError> {
    let field = bytes
        .get(at..at + 16)
        .ok_or_else(|| malformed("Mach-O segment name is truncated"))?;
    Ok(&field[..field.iter().position(|byte| *byte == 0).unwrap_or(16)])
}

fn usize_of(value: u64) -> Result<usize, BundleError> {
    usize::try_from(value).map_err(|_| malformed("Mach-O file offset does not fit this host"))
}

fn parse(executable: &[u8]) -> Result<Option<MachO>, BundleError> {
    if executable.len() < 4 || read_u32(executable, 0)? != MH_MAGIC_64 {
        return Ok(None);
    }
    if executable.len() < HEADER_SIZE {
        return Err(malformed("the Mach-O 64-bit header is truncated"));
    }
    if read_u32(executable, 12)? != MH_EXECUTE {
        return Err(malformed("the Mach-O input is not an executable"));
    }
    let cpu = read_i32(executable, 4)?;
    if cpu != CPU_TYPE_X86_64 && cpu != CPU_TYPE_ARM64 {
        return Err(malformed(format!(
            "Mach-O CPU type 0x{:x} is not a shipping Darwin architecture",
            cpu as u32
        )));
    }
    let ncmds = read_u32(executable, 16)? as usize;
    let sizeofcmds = read_u32(executable, 20)? as usize;
    let commands_end = HEADER_SIZE
        .checked_add(sizeofcmds)
        .ok_or_else(|| malformed("the Mach-O load-command size overflows"))?;
    if commands_end > executable.len() {
        return Err(malformed("the Mach-O load-command table is truncated"));
    }

    let mut commands = Vec::with_capacity(ncmds);
    let mut offset = HEADER_SIZE;
    for index in 0..ncmds {
        if offset + 8 > commands_end {
            return Err(malformed(format!(
                "Mach-O load command {index} has no complete header"
            )));
        }
        let kind = read_u32(executable, offset)?;
        let size = read_u32(executable, offset + 4)? as usize;
        if size < 8 || !size.is_multiple_of(8) || offset + size > commands_end {
            return Err(malformed(format!(
                "Mach-O load command {index} has invalid size {size}"
            )));
        }
        commands.push(Command { kind, size, offset });
        offset += size;
    }
    if offset != commands_end {
        return Err(malformed("Mach-O ncmds and sizeofcmds disagree"));
    }

    let mut linkedit = None;
    let mut text = None;
    let mut blitsen = None;
    let mut signature = None;
    let mut first_file_data = u64::MAX;
    for command in &commands {
        if command.kind == LC_CODE_SIGNATURE {
            signature = Some(*command);
        }
        if command.kind != LC_SEGMENT_64 {
            continue;
        }
        if command.size < SEGMENT_COMMAND_SIZE {
            return Err(malformed("an LC_SEGMENT_64 command is truncated"));
        }
        let segment = Segment {
            command: *command,
            vmaddr: read_u64(executable, command.offset + 24)?,
            fileoff: read_u64(executable, command.offset + 40)?,
            filesize: read_u64(executable, command.offset + 48)?,
            nsects: read_u32(executable, command.offset + 64)? as usize,
        };
        if command.size < SEGMENT_COMMAND_SIZE + segment.nsects * SECTION_SIZE {
            return Err(malformed(
                "an LC_SEGMENT_64 command is shorter than its section table",
            ));
        }
        match name(executable, command.offset + 8)? {
            b"__TEXT" => text = Some(segment),
            b"__LINKEDIT" => linkedit = Some(segment),
            b"__BLITSEN" => blitsen = Some(segment),
            _ => {}
        }
        for index in 0..segment.nsects {
            let section = command.offset + SEGMENT_COMMAND_SIZE + index * SECTION_SIZE;
            let size = read_u64(executable, section + 40)?;
            let fileoff = read_u32(executable, section + 48)? as u64;
            if size > 0 && fileoff > 0 {
                first_file_data = first_file_data.min(fileoff);
            }
        }
    }
    let linkedit = linkedit.ok_or_else(|| malformed("the Mach-O __LINKEDIT segment is missing"))?;
    let text = text.ok_or_else(|| malformed("the Mach-O __TEXT segment is missing"))?;
    let linkedit_end = linkedit
        .fileoff
        .checked_add(linkedit.filesize)
        .ok_or_else(|| malformed("the Mach-O __LINKEDIT size overflows"))?;
    if usize_of(linkedit_end)? != executable.len() {
        return Err(malformed(
            "Mach-O __LINKEDIT is not the final bytes of the input runtime",
        ));
    }

    let mut signature_offset = executable.len();
    if let Some(command) = signature {
        if command.size != 16 {
            return Err(malformed("Mach-O LC_CODE_SIGNATURE has an invalid size"));
        }
        signature_offset = read_u32(executable, command.offset + 8)? as usize;
        let signature_size = read_u32(executable, command.offset + 12)? as usize;
        if signature_offset < usize_of(linkedit.fileoff)?
            || signature_offset.checked_add(signature_size) != Some(executable.len())
        {
            return Err(malformed(
                "the inherited Mach-O code signature is not the final __LINKEDIT object",
            ));
        }
    }

    let extra_commands = NEW_SEGMENT_COMMAND_SIZE + if signature.is_some() { 0 } else { 16 };
    if first_file_data == u64::MAX {
        first_file_data = linkedit.fileoff;
    }
    let first_file_data = usize_of(first_file_data)?;
    if commands_end + extra_commands > first_file_data {
        return Err(malformed(format!(
            "the Mach-O header has {} bytes of load-command padding, but embedding the payload needs {extra_commands}",
            first_file_data.saturating_sub(commands_end)
        )));
    }
    if executable[commands_end..commands_end + extra_commands]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(malformed(
            "the Mach-O load-command padding that would hold __BLITSEN is not empty",
        ));
    }

    Ok(Some(MachO {
        commands,
        linkedit,
        text,
        blitsen,
        signature,
        signature_offset,
        page_size: if cpu == CPU_TYPE_ARM64 {
            0x4000
        } else {
            0x1000
        },
    }))
}

pub(super) fn payload_offset(executable: &[u8]) -> Result<Option<u64>, BundleError> {
    Ok(parse(executable)?.map(|macho| macho.linkedit.fileoff))
}

fn align(value: usize, boundary: usize) -> Result<usize, BundleError> {
    value
        .checked_add(boundary - 1)
        .map(|value| value / boundary * boundary)
        .ok_or_else(|| malformed("Mach-O alignment overflows"))
}

fn shift_u32(command: &mut [u8], at: usize, amount: usize, lower: usize, upper: usize) {
    let value = u32::from_le_bytes(command[at..at + 4].try_into().expect("four bytes")) as usize;
    if value != 0 && value >= lower && value < upper {
        write_u32(command, at, (value + amount) as u32);
    }
}

fn shift_u64(command: &mut [u8], at: usize, amount: usize, lower: usize, upper: usize) {
    let value = u64::from_le_bytes(command[at..at + 8].try_into().expect("eight bytes")) as usize;
    if value != 0 && value >= lower && value < upper {
        write_u64(command, at, (value + amount) as u64);
    }
}

fn patch_command(
    command: &mut [u8],
    kind: u32,
    amount: usize,
    lower: usize,
    upper: usize,
    signature_offset: usize,
    signature_size: usize,
) {
    match kind {
        LC_SEGMENT_64 => {
            let nsects = u32::from_le_bytes(command[64..68].try_into().expect("four bytes"));
            for index in 0..nsects as usize {
                let section = SEGMENT_COMMAND_SIZE + index * SECTION_SIZE;
                shift_u32(command, section + 48, amount, lower, upper);
                shift_u32(command, section + 56, amount, lower, upper);
            }
        }
        LC_SYMTAB => {
            shift_u32(command, 8, amount, lower, upper);
            shift_u32(command, 16, amount, lower, upper);
        }
        LC_DYSYMTAB => {
            for at in [32, 40, 48, 56, 64, 72] {
                shift_u32(command, at, amount, lower, upper);
            }
        }
        LC_DYLD_INFO | LC_DYLD_INFO_ONLY => {
            for at in [8, 16, 24, 32, 40] {
                shift_u32(command, at, amount, lower, upper);
            }
        }
        LC_SEGMENT_SPLIT_INFO
        | LC_FUNCTION_STARTS
        | LC_DATA_IN_CODE
        | LC_DYLIB_CODE_SIGN_DRS
        | LC_ATOM_INFO
        | LC_LINKER_OPTIMIZATION_HINT
        | LC_DYLD_EXPORTS_TRIE
        | LC_DYLD_CHAINED_FIXUPS
        | LC_FUNCTION_VARIANTS
        | LC_FUNCTION_VARIANT_FIXUPS => shift_u32(command, 8, amount, lower, upper),
        LC_TWOLEVEL_HINTS | LC_ENCRYPTION_INFO | LC_ENCRYPTION_INFO_64 => {
            shift_u32(command, 8, amount, lower, upper);
        }
        LC_NOTE => shift_u64(command, 24, amount, lower, upper),
        LC_FILESET_ENTRY => shift_u64(command, 16, amount, lower, upper),
        _ => {}
    }
    if kind == LC_CODE_SIGNATURE {
        write_u32(command, 8, signature_offset as u32);
        write_u32(command, 12, signature_size as u32);
    }
}

fn segment_command(vmaddr: u64, vmsize: usize, fileoff: usize, filesize: usize) -> Vec<u8> {
    let mut command = vec![0_u8; NEW_SEGMENT_COMMAND_SIZE];
    write_u32(&mut command, 0, LC_SEGMENT_64);
    write_u32(&mut command, 4, NEW_SEGMENT_COMMAND_SIZE as u32);
    fixed_name(&mut command, 8, b"__BLITSEN");
    write_u64(&mut command, 24, vmaddr);
    write_u64(&mut command, 32, vmsize as u64);
    write_u64(&mut command, 40, fileoff as u64);
    write_u64(&mut command, 48, filesize as u64);
    write_u32(&mut command, 56, 1);
    write_u32(&mut command, 60, 1);
    command
}

fn signature_size(code_limit: usize) -> usize {
    let slots = code_limit.div_ceil(SIGN_PAGE_SIZE);
    12 + 8 + CODE_DIRECTORY_SIZE + SIGNATURE_IDENTIFIER.len() + slots * 32
}

fn ad_hoc_signature(bytes: &[u8], code_limit: usize, text: Segment) -> Vec<u8> {
    let slots = code_limit.div_ceil(SIGN_PAGE_SIZE);
    let ident_offset = CODE_DIRECTORY_SIZE;
    let hash_offset = ident_offset + SIGNATURE_IDENTIFIER.len();
    let directory_size = hash_offset + slots * 32;
    let total = 12 + 8 + directory_size;
    let mut result = vec![0_u8; total];
    write_be_u32(&mut result, 0, CSMAGIC_EMBEDDED_SIGNATURE);
    write_be_u32(&mut result, 4, total as u32);
    write_be_u32(&mut result, 8, 1);
    write_be_u32(&mut result, 12, 0);
    write_be_u32(&mut result, 16, 20);
    write_be_u32(&mut result, 20, CSMAGIC_CODEDIRECTORY);
    write_be_u32(&mut result, 24, directory_size as u32);
    write_be_u32(&mut result, 28, 0x20400);
    write_be_u32(&mut result, 32, 0x20002);
    write_be_u32(&mut result, 36, hash_offset as u32);
    write_be_u32(&mut result, 40, ident_offset as u32);
    write_be_u32(&mut result, 44, 0);
    write_be_u32(&mut result, 48, slots as u32);
    write_be_u32(&mut result, 52, code_limit as u32);
    result[56] = 32;
    result[57] = 2;
    result[59] = 12;
    write_be_u64(&mut result, 84, text.fileoff);
    write_be_u64(&mut result, 92, text.filesize);
    write_be_u64(&mut result, 100, 1);
    result[20 + ident_offset..20 + ident_offset + SIGNATURE_IDENTIFIER.len()]
        .copy_from_slice(SIGNATURE_IDENTIFIER);
    let mut hash_at = 20 + hash_offset;
    for page in bytes[..code_limit].chunks(SIGN_PAGE_SIZE) {
        let digest = Sha256::digest(page);
        result[hash_at..hash_at + 32].copy_from_slice(&digest);
        hash_at += 32;
    }
    result
}

pub(super) fn inject(
    executable: &[u8],
    section_data: &[u8],
) -> Result<Option<Vec<u8>>, BundleError> {
    let Some(macho) = parse(executable)? else {
        return Ok(None);
    };
    if macho.blitsen.is_some() {
        return Err(malformed(
            "the Mach-O runtime already contains a __BLITSEN segment",
        ));
    }
    if section_data.len() < TRAILER_SIZE {
        return Err(malformed("the Mach-O bundle has no complete trailer"));
    }
    let segment_filesize = align(section_data.len(), macho.page_size)?;
    let linkedit_at = usize_of(macho.linkedit.fileoff)?;
    let linkedit_data = &executable[linkedit_at..macho.signature_offset];
    let new_linkedit_offset = linkedit_at + segment_filesize;
    let new_signature_offset = new_linkedit_offset + linkedit_data.len();
    let new_signature_size = signature_size(new_signature_offset);
    let new_linkedit_filesize = linkedit_data.len() + new_signature_size;
    let new_linkedit_vmsize = align(new_linkedit_filesize.max(macho.page_size), macho.page_size)?;

    let mut header = executable[..HEADER_SIZE].to_vec();
    let added_commands = 1 + usize::from(macho.signature.is_none());
    let old_ncmds = read_u32(&header, 16)?;
    let old_sizeofcmds = read_u32(&header, 20)?;
    write_u32(&mut header, 16, old_ncmds + added_commands as u32);
    write_u32(
        &mut header,
        20,
        old_sizeofcmds
            + NEW_SEGMENT_COMMAND_SIZE as u32
            + if macho.signature.is_none() { 16 } else { 0 },
    );

    let embedded = segment_command(
        macho.linkedit.vmaddr,
        segment_filesize,
        linkedit_at,
        segment_filesize,
    );
    let mut rebuilt_commands = Vec::new();
    let mut inserted = false;
    for record in &macho.commands {
        if record.offset == macho.linkedit.command.offset {
            rebuilt_commands.extend_from_slice(&embedded);
            if macho.signature.is_none() {
                let mut signature = [0_u8; 16];
                write_u32(&mut signature, 0, LC_CODE_SIGNATURE);
                write_u32(&mut signature, 4, 16);
                write_u32(&mut signature, 8, new_signature_offset as u32);
                write_u32(&mut signature, 12, new_signature_size as u32);
                rebuilt_commands.extend_from_slice(&signature);
            }
            inserted = true;
        }
        let mut command = executable[record.offset..record.offset + record.size].to_vec();
        patch_command(
            &mut command,
            record.kind,
            segment_filesize,
            linkedit_at,
            macho.signature_offset,
            new_signature_offset,
            new_signature_size,
        );
        if record.offset == macho.linkedit.command.offset {
            write_u64(
                &mut command,
                24,
                macho.linkedit.vmaddr + segment_filesize as u64,
            );
            write_u64(&mut command, 32, new_linkedit_vmsize as u64);
            write_u64(&mut command, 40, new_linkedit_offset as u64);
            write_u64(&mut command, 48, new_linkedit_filesize as u64);
        }
        rebuilt_commands.extend_from_slice(&command);
    }
    if !inserted {
        return Err(malformed(
            "could not place Mach-O __BLITSEN before __LINKEDIT",
        ));
    }

    let new_commands_end = HEADER_SIZE + rebuilt_commands.len();
    let mut unsigned = Vec::with_capacity(new_signature_offset);
    unsigned.extend_from_slice(&header);
    unsigned.extend_from_slice(&rebuilt_commands);
    unsigned.extend_from_slice(&executable[new_commands_end..linkedit_at]);
    let trailer_at = section_data.len() - TRAILER_SIZE;
    unsigned.extend_from_slice(&section_data[..trailer_at]);
    unsigned.resize(unsigned.len() + segment_filesize - section_data.len(), 0);
    unsigned.extend_from_slice(&section_data[trailer_at..]);
    unsigned.extend_from_slice(linkedit_data);
    if unsigned.len() != new_signature_offset {
        return Err(malformed(
            "the rebuilt Mach-O segment offsets do not agree with the bytes written",
        ));
    }
    let signature = ad_hoc_signature(&unsigned, new_signature_offset, macho.text);
    if signature.len() != new_signature_size {
        return Err(malformed(
            "the Mach-O ad-hoc signature size changed while it was being written",
        ));
    }
    unsigned.extend_from_slice(&signature);
    Ok(Some(unsigned))
}
