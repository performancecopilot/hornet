//! Native-endian read/write helpers
//!
//! MMV files are a same-host IPC mechanism, written by an instrumented process
//! and read back via mmap by `pmdammv` running on that same machine, never
//! transferred across hosts. The mmv(5) spec and the reference `libpcp_mmv` C
//! implementation both write plain native integers with no conversion, so these
//! methods use the host's native byte order rather than a fixed one.

use std::io::{self, Read, Write};

pub trait ReadBytesExt: Read {
    fn read_u8(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    fn read_i32(&mut self) -> io::Result<i32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(i32::from_ne_bytes(buf))
    }

    fn read_u32(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_ne_bytes(buf))
    }

    fn read_i64(&mut self) -> io::Result<i64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(i64::from_ne_bytes(buf))
    }

    fn read_u64(&mut self) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_ne_bytes(buf))
    }
}

impl<R: Read + ?Sized> ReadBytesExt for R {}

pub trait WriteBytesExt: Write {
    fn write_i32(&mut self, n: i32) -> io::Result<()> {
        self.write_all(&n.to_ne_bytes())
    }

    fn write_u32(&mut self, n: u32) -> io::Result<()> {
        self.write_all(&n.to_ne_bytes())
    }

    fn write_i64(&mut self, n: i64) -> io::Result<()> {
        self.write_all(&n.to_ne_bytes())
    }

    fn write_u64(&mut self, n: u64) -> io::Result<()> {
        self.write_all(&n.to_ne_bytes())
    }
}

impl<W: Write + ?Sized> WriteBytesExt for W {}
