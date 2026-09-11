pub mod batch;
pub mod bytes;
pub mod int64;
pub mod path_delta;
pub mod path_meta;
pub mod u256;
pub mod uint64;

pub type Result<T> = std::result::Result<T, &'static str>;

pub(super) struct Writer<'a> {
    output: &'a mut [u8],
    offset: usize,
}

impl<'a> Writer<'a> {
    pub(super) fn new(output: &'a mut [u8]) -> Self {
        Self { output, offset: 0 }
    }

    #[inline]
    pub(super) fn write_u8(&mut self, value: u8) -> Result<()> {
        self.write_bytes(&[value])
    }

    #[inline]
    pub(super) fn write_u64(&mut self, value: u64) -> Result<()> {
        self.write_bytes(&value.to_le_bytes())
    }

    #[inline]
    pub(super) fn write_i64(&mut self, value: i64) -> Result<()> {
        self.write_bytes(&value.to_le_bytes())
    }

    #[inline]
    pub(super) fn write_bytes(&mut self, value: &[u8]) -> Result<()> {
        let end = self
            .offset
            .checked_add(value.len())
            .ok_or("encoded size overflow")?;
        let target = self
            .output
            .get_mut(self.offset..end)
            .ok_or("output buffer too small")?;
        target.copy_from_slice(value);
        self.offset = end;
        Ok(())
    }

    pub(super) fn finish(self) -> usize {
        self.offset
    }
}

pub(super) struct Reader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    #[inline]
    pub(super) fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    #[inline]
    pub(super) fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.read_bytes(8)?.try_into().map_err(|_| "invalid u64")?,
        ))
    }

    #[inline]
    pub(super) fn read_i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(
            self.read_bytes(8)?.try_into().map_err(|_| "invalid i64")?,
        ))
    }

    #[inline]
    pub(super) fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.read_bytes(N)?
            .try_into()
            .map_err(|_| "invalid fixed-width value")
    }

    #[inline]
    pub(super) fn read_bytes(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or("encoded size overflow")?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or("unexpected end of input")?;
        self.offset = end;
        Ok(value)
    }

    pub(super) fn remaining(&self) -> usize {
        self.input.len() - self.offset
    }

    pub(super) fn finish(self) -> Result<()> {
        if self.offset == self.input.len() {
            Ok(())
        } else {
            Err("trailing bytes")
        }
    }
}

#[cfg(test)]
mod tests;
