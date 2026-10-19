//! The ZIP subset Fieldwork reads and writes: stored members, UTF-8 names, fixed
//! timestamps, no extra fields, no ZIP64 (see fieldwork-format §2).

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const DOS_DATE_1980_01_01: u16 = 0x0021;

fn le16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn le32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 512 {
        return Err(format!("member path length {} is outside 1..=512", path.len()));
    }
    if path.starts_with('/') || path.contains(['\\', ':', '\0']) {
        return Err(format!("member path {path:?} has a forbidden character"));
    }
    if path.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(format!("member path {path:?} has an empty, . or .. component"));
    }
    Ok(())
}

pub fn write(members: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (path, data) in members {
        validate_path(path)?;
        if data.len() >= u32::MAX as usize {
            return Err(format!("{path} is too large for this format version"));
        }
        let crc = crc32fast::hash(data);
        let offset = u32::try_from(out.len()).map_err(|_| "archive too large")?;
        let name = path.as_bytes();
        le32(&mut out, LOCAL);
        le16(&mut out, 20);
        le16(&mut out, 0x0800);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, DOS_DATE_1980_01_01);
        le32(&mut out, crc);
        le32(&mut out, data.len() as u32);
        le32(&mut out, data.len() as u32);
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        le32(&mut central, CENTRAL);
        le16(&mut central, 20);
        le16(&mut central, 20);
        le16(&mut central, 0x0800);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, DOS_DATE_1980_01_01);
        le32(&mut central, crc);
        le32(&mut central, data.len() as u32);
        le32(&mut central, data.len() as u32);
        le16(&mut central, name.len() as u16);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le32(&mut central, 0);
        le32(&mut central, offset);
        central.extend_from_slice(name);
    }
    let central_offset = u32::try_from(out.len()).map_err(|_| "archive too large")?;
    out.extend_from_slice(&central);
    le32(&mut out, END);
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, members.len() as u16);
    le16(&mut out, members.len() as u16);
    le32(&mut out, central.len() as u32);
    le32(&mut out, central_offset);
    le16(&mut out, 0);
    Ok(out)
}

fn rd16(b: &[u8], p: usize) -> Result<u16, String> {
    b.get(p..p + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "truncated archive".to_string())
}
fn rd32(b: &[u8], p: usize) -> Result<u32, String> {
    b.get(p..p + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| "truncated archive".to_string())
}

/// Reads an archive with the same checks Fieldwork's reader applies.
pub fn read(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if b.len() < 22 {
        return Err("shorter than an end-of-central-directory record".into());
    }
    let eocd = (0..=b.len() - 22).rev().find(|&p| rd32(b, p).ok() == Some(END)).ok_or("no end-of-central-directory record")?;
    if rd16(b, eocd + 4)? != 0 || rd16(b, eocd + 6)? != 0 {
        return Err("multi-disk archives are not supported".into());
    }
    let count = rd16(b, eocd + 10)? as usize;
    let central_offset = rd32(b, eocd + 16)? as usize;
    if count > 10_000 {
        return Err("too many members".into());
    }
    let mut members = Vec::with_capacity(count);
    let mut p = central_offset;
    let mut total = 0u64;
    for _ in 0..count {
        if rd32(b, p)? != CENTRAL {
            return Err("bad central directory entry".into());
        }
        let flags = rd16(b, p + 8)?;
        let method = rd16(b, p + 10)?;
        let crc = rd32(b, p + 16)?;
        let csize = rd32(b, p + 20)?;
        let usize_ = rd32(b, p + 24)?;
        let name_len = rd16(b, p + 28)? as usize;
        let extra_len = rd16(b, p + 30)? as usize;
        let comment_len = rd16(b, p + 32)? as usize;
        let local = rd32(b, p + 42)? as usize;
        let name = std::str::from_utf8(b.get(p + 46..p + 46 + name_len).ok_or("truncated name")?).map_err(|_| "member name is not UTF-8")?.to_string();
        if flags & 1 != 0 || method != 0 || csize != usize_ || csize == u32::MAX {
            return Err(format!("{name}: encrypted, compressed or ZIP64 members are not supported"));
        }
        validate_path(&name)?;
        if rd32(b, local)? != LOCAL {
            return Err(format!("{name}: bad local header"));
        }
        let lname_len = rd16(b, local + 26)? as usize;
        let lextra_len = rd16(b, local + 28)? as usize;
        if b.get(local + 30..local + 30 + lname_len) != Some(name.as_bytes()) {
            return Err(format!("{name}: local name differs from the central directory"));
        }
        let start = local + 30 + lname_len + lextra_len;
        let end = start + csize as usize;
        if end > central_offset {
            return Err(format!("{name}: data runs into the central directory"));
        }
        let data = b[start..end].to_vec();
        if crc32fast::hash(&data) != crc {
            return Err(format!("{name}: CRC-32 mismatch"));
        }
        total += data.len() as u64;
        if total > 2 << 30 {
            return Err("archive expands beyond 2 GiB".into());
        }
        if members.iter().any(|(n, _): &(String, Vec<u8>)| *n == name) {
            return Err(format!("{name}: duplicate member"));
        }
        members.push((name, data));
        p += 46 + name_len + extra_len + comment_len;
    }
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_path_rules() {
        let members = vec![("manifest.json".to_string(), b"{}".to_vec()), ("extensions/a.b/c.json".to_string(), vec![1, 2, 3])];
        let z = write(&members).unwrap();
        assert_eq!(read(&z).unwrap(), members);
        assert_eq!(&z[..4], b"PK\x03\x04");
        for bad in ["", "/abs", "a//b", "./a", "a/../b", "a\\b", "c:d"] {
            assert!(validate_path(bad).is_err(), "{bad:?}");
        }
        let mut corrupt = z.clone();
        corrupt[40] ^= 0xff;
        assert!(read(&corrupt).is_err());
    }
}
