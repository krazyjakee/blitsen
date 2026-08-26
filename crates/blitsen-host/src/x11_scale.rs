//! The global toolkit scale published by X11 desktops through XSettings.

use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

const SCALE_NAME: &[u8] = b"Gdk/WindowScalingFactor";

/// Returns the XSettings toolkit scale when this is an X11 session.
pub(crate) fn system_scale_factor() -> Option<f64> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind != "x11")
    {
        return None;
    }
    let (connection, screen) = x11rb::connect(None).ok()?;
    let selection = connection
        .intern_atom(false, format!("_XSETTINGS_S{screen}").as_bytes())
        .ok()?
        .reply()
        .ok()?
        .atom;
    let settings = connection
        .intern_atom(false, b"_XSETTINGS_SETTINGS")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let owner = connection
        .get_selection_owner(selection)
        .ok()?
        .reply()
        .ok()?
        .owner;
    if owner == 0 {
        return None;
    }
    let bytes = connection
        .get_property(false, owner, settings, AtomEnum::ANY, 0, u32::MAX)
        .ok()?
        .reply()
        .ok()?
        .value;
    parse_scale(&bytes).map(f64::from)
}

fn parse_scale(bytes: &[u8]) -> Option<u32> {
    let little = match *bytes.first()? {
        0 => true,
        1 => false,
        _ => return None,
    };
    let read_u16 = |bytes: &[u8]| {
        let value: [u8; 2] = bytes.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(value)
        } else {
            u16::from_be_bytes(value)
        })
    };
    let read_u32 = |bytes: &[u8]| {
        let value: [u8; 4] = bytes.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(value)
        } else {
            u32::from_be_bytes(value)
        })
    };
    let count = read_u32(bytes.get(8..12)?)?;
    let mut offset = 12;
    for _ in 0..count {
        let kind = *bytes.get(offset)?;
        let name_len = usize::from(read_u16(bytes.get(offset + 2..offset + 4)?)?);
        offset += 4;
        let name = bytes.get(offset..offset + name_len)?;
        offset = (offset + name_len + 3) & !3;
        offset += 4; // setting serial
        match kind {
            0 => {
                let value = read_u32(bytes.get(offset..offset + 4)?)?;
                offset += 4;
                if name == SCALE_NAME {
                    return (value > 0).then_some(value);
                }
            }
            1 => {
                let len = read_u32(bytes.get(offset..offset + 4)?)? as usize;
                offset = (offset + 4 + len + 3) & !3;
            }
            2 => offset += 8,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gdk_scale_from_xsettings() {
        let mut bytes = vec![0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0];
        bytes.extend([0, 0, 23, 0]);
        bytes.extend(SCALE_NAME);
        bytes.push(0);
        bytes.extend([7, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(parse_scale(&bytes), Some(2));
    }

    #[test]
    fn reads_big_endian_xsettings() {
        let mut bytes = vec![1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1];
        bytes.extend([0, 0, 0, 23]);
        bytes.extend(SCALE_NAME);
        bytes.push(0);
        bytes.extend([0, 0, 0, 7, 0, 0, 0, 2]);
        assert_eq!(parse_scale(&bytes), Some(2));
    }
}
