//! The small Mach-O read path retained in shipped runtimes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use super::{BundleError, malformed};

const MH_MAGIC_64: u32 = 0xfeedfacf;
const LC_SEGMENT_64: u32 = 0x19;
const HEADER_SIZE: usize = 32;
const SEGMENT_COMMAND_SIZE: usize = 72;

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, BundleError> {
    let value = bytes
        .get(at..at + 4)
        .ok_or_else(|| malformed("Mach-O integer is truncated"))?;
    Ok(u32::from_le_bytes(value.try_into().expect("four bytes")))
}

fn read_u64(bytes: &[u8], at: usize) -> Result<u64, BundleError> {
    let value = bytes
        .get(at..at + 8)
        .ok_or_else(|| malformed("Mach-O integer is truncated"))?;
    Ok(u64::from_le_bytes(value.try_into().expect("eight bytes")))
}

fn name(bytes: &[u8], at: usize) -> Result<&[u8], BundleError> {
    let field = bytes
        .get(at..at + 16)
        .ok_or_else(|| malformed("Mach-O segment name is truncated"))?;
    Ok(&field[..field.iter().position(|byte| *byte == 0).unwrap_or(16)])
}

pub(super) fn payload_section(file: &mut File) -> Result<Option<(u64, u64)>, BundleError> {
    let mut header = [0_u8; HEADER_SIZE];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    if read_u32(&header, 0)? != MH_MAGIC_64 {
        return Ok(None);
    }
    let commands_size = read_u32(&header, 20)? as usize;
    let mut commands = vec![0_u8; commands_size];
    file.read_exact(&mut commands)?;
    let ncmds = read_u32(&header, 16)? as usize;
    let mut offset = 0;
    for index in 0..ncmds {
        if offset + 8 > commands.len() {
            return Err(malformed(format!(
                "Mach-O load command {index} has no complete header"
            )));
        }
        let kind = read_u32(&commands, offset)?;
        let size = read_u32(&commands, offset + 4)? as usize;
        if size < 8 || !size.is_multiple_of(8) || offset + size > commands.len() {
            return Err(malformed(format!(
                "Mach-O load command {index} has invalid size {size}"
            )));
        }
        if kind == LC_SEGMENT_64 && name(&commands, offset + 8)? == b"__BLITSEN" {
            if size != SEGMENT_COMMAND_SIZE || read_u32(&commands, offset + 64)? != 0 {
                return Err(malformed(
                    "the Mach-O __BLITSEN segment has an unexpected section table",
                ));
            }
            return Ok(Some((
                read_u64(&commands, offset + 40)?,
                read_u64(&commands, offset + 48)?,
            )));
        }
        offset += size;
    }
    Ok(None)
}
