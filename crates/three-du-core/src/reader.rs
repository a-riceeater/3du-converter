//! Bounds-checked big-endian cursor over an untrusted byte buffer.

use crate::error::DecodeError;

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn position(&self) -> u64 {
        self.pos as u64
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn take(&mut self, n: usize, context: &'static str) -> Result<&'a [u8], DecodeError> {
        if n > self.remaining() {
            return Err(DecodeError::Truncated {
                context,
                offset: self.position(),
                needed: (n - self.remaining()) as u64,
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn array<const N: usize>(&mut self, context: &'static str) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, context)?);
        Ok(out)
    }

    pub fn u8(&mut self, context: &'static str) -> Result<u8, DecodeError> {
        Ok(self.take(1, context)?[0])
    }

    pub fn u16(&mut self, context: &'static str) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array(context)?))
    }

    pub fn u32(&mut self, context: &'static str) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array(context)?))
    }

    pub fn f32(&mut self, context: &'static str) -> Result<f32, DecodeError> {
        Ok(f32::from_be_bytes(self.array(context)?))
    }

    pub fn f64(&mut self, context: &'static str) -> Result<f64, DecodeError> {
        Ok(f64::from_be_bytes(self.array(context)?))
    }

    /// A `u16` byte length followed by that many bytes of text. Text is UTF-8 when valid,
    /// otherwise each byte is read as Windows-1252/Latin-1 (the encoding of legacy files).
    pub fn string(&mut self, context: &'static str) -> Result<String, DecodeError> {
        let len = self.u16(context)? as usize;
        let bytes = self.take(len, context)?;
        Ok(decode_text(bytes))
    }

    /// Reads a `u16` element count and checks that `count * min_record_size` bytes remain,
    /// so a corrupt count can never trigger a huge allocation.
    pub fn count(&mut self, context: &'static str, min_record_size: usize) -> Result<usize, DecodeError> {
        let offset = self.position();
        let n = self.u16(context)? as usize;
        if n.saturating_mul(min_record_size) > self.remaining() {
            return Err(DecodeError::Invalid {
                context,
                offset,
                detail: format!("count {n} cannot fit in the {} remaining byte(s)", self.remaining()),
            });
        }
        Ok(n)
    }
}

pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian_values() {
        let data = [0x00, 0x2d, 0x42, 0x5c, 0x00, 0x00, 0x00, 0x02, b'h', b'i'];
        let mut r = Reader::new(&data);
        assert_eq!(r.u16("t").unwrap(), 45);
        assert_eq!(r.f32("t").unwrap(), 55.0);
        assert_eq!(r.string("t").unwrap(), "hi");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn truncation_reports_offset() {
        let mut r = Reader::new(&[0x00, 0x05, b'a']);
        match r.string("label") {
            Err(DecodeError::Truncated { offset, needed, .. }) => {
                assert_eq!(offset, 2);
                assert_eq!(needed, 4);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn count_rejects_impossible_lengths() {
        let mut r = Reader::new(&[0xff, 0xff, 0, 0]);
        assert!(matches!(r.count("frames", 4), Err(DecodeError::Invalid { .. })));
    }

    #[test]
    fn latin1_fallback() {
        assert_eq!(decode_text(&[0x43, 0x61, 0x66, 0xe9]), "Café");
    }
}
