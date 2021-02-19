use obd::Uds;
use std::cmp;
use thiserror::Error;

static MZR_KEY: &'static str = "MazdA";


const UDS_REQ_REQUESTDOWNLOAD: u8 = 0x34;
const UDS_REQ_TRANSFERDATA: u8 = 0x36;

#[derive(Error, Debug)]
pub enum MzrError {
    #[error("received empty packet")]
    EmptyPacket,
    #[error("flash memory must be erased before programming")]
    NotErased,
    #[error("transmission error: {0}")]
    Obd(#[from] obd::Error),
}

/// Trait for MZR-DISI specific operations.
pub trait MzrBus {
    fn authenticate(&mut self, session_id: u8) -> Result<(), MzrError>;
    fn request_download(&mut self, offset: u32, length: u32) -> Result<(), MzrError>;
    fn transfer_data(&mut self, data: &[u8]) -> Result<(), MzrError>;
}


impl<T> MzrBus for T
where
    T: Uds,
{
    fn authenticate(&mut self, session_id: u8) -> Result<(), MzrError> {
        self.set_diagnostic_session(0x7e0, session_id)?;
        let seed = self.request_security_seed(0x7e0)?;
        let key = generate_key(MZR_KEY, 0xC541A9, &seed);
        self.request_security_key(0x7e0, &key)?;

        Ok(())
    }

    fn request_download(&mut self, offset: u32, length: u32) -> Result<(), MzrError> {
        let mut req = [0; 8];
        req[0] = ((offset & 0xFF000000) >> 24) as u8;
        req[1] = ((offset & 0xFF0000) >> 16) as u8;
        req[2] = ((offset & 0xFF00) >> 8) as u8;
        req[3] = (offset & 0xFF) as u8;

        req[4] = ((length & 0xFF000000) >> 24) as u8;
        req[5] = ((length & 0xFF0000) >> 16) as u8;
        req[6] = ((length & 0xFF00) >> 8) as u8;
        req[7] = (length & 0xFF) as u8;

        self.query_uds(0x7e0, UDS_REQ_REQUESTDOWNLOAD, &req)?;
        Ok(())
    }

    fn transfer_data(&mut self, data: &[u8]) -> Result<(), MzrError> {
        self.query_uds(0x7e0, UDS_REQ_TRANSFERDATA, data)?;
        Ok(())
    }
}

pub enum DownloadState {
    // Progress (length downloaded)
    InProgress(usize),
    Completed,
}

pub struct Downloader<'a, M: 'a + Uds> {
    offset: u32,
    remaining: usize,
    data: Vec<u8>,
    bus: &'a mut M,
}

impl<'a, M: 'a + Uds> Downloader<'a, M> {
    pub fn new(bus: &'a mut M) -> Downloader<'a, M> {
        Downloader {
            offset: 0,
            remaining: 1024 * 1024,
            data: Vec::with_capacity(1024 * 1024),
            bus,
        }
    }

    /// Returns the total download size
    pub fn total_size(&self) -> usize {
        1024 * 1024
    }

    pub fn start(&mut self) -> Result<(), MzrError> {
        self.bus.authenticate(0x87)
    }

    /// Next download step
    pub fn step(&mut self) -> Result<DownloadState, MzrError> {
        if self.remaining == 0 {
            return Ok(DownloadState::Completed);
        }
        let section = self.bus.read_memory_address(
            0x7e0,
            self.offset,
            cmp::min(self.remaining, 0xFFE) as u16,
        )?;
        if section.is_empty() {
            return Err(MzrError::EmptyPacket);
        }

        // Add response to buffer
        self.data.extend_from_slice(&section);
        self.offset += section.len() as u32;
        self.remaining -= section.len();

        if self.remaining > 0 {
            Ok(DownloadState::InProgress(self.data.len()))
        } else {
            Ok(DownloadState::Completed)
        }
    }

    pub fn take_data(self) -> Vec<u8> {
        self.data
    }
}



pub enum ProgrammerState {
    // Progress (length uploaded)
    InProgress(usize),
    Completed,
}

pub struct Programmer<'a, M: 'a + Uds> {
    offset: u32,
    position: usize,
    data: Vec<u8>,
    bus: &'a mut M,
    erased: bool,
}

impl<'a, M: 'a + Uds> Programmer<'a, M> {
    pub fn new(bus: &'a mut M, offset: u32, data: Vec<u8>) -> Programmer<'a, M> {
        Programmer {
            offset,
            position: 0,
            data,
            bus,
            erased: false,
        }
    }

    /// Returns the total data length
    pub fn total_size(&self) -> usize {
        self.data.len()
    }

    // This function MUST be called before sending data
    pub fn start(&mut self) -> Result<(), MzrError> {
        self.bus.authenticate(0x85)?;
        // Erase flash memory
        self.bus.query_uds(0x7e0, 0xB1, &[0x00, 0xB2, 0x00])?;
        self.bus.request_download(self.offset, self.data.len() as u32 - self.position as u32)?;
        self.erased = true;
        Ok(())
    }

    /// Next programming step
    pub fn step(&mut self) -> Result<ProgrammerState, MzrError> {
        if !self.erased {
            return Err(MzrError::NotErased);
        }
        if self.position == self.data.len() {
            return Ok(ProgrammerState::Completed);
        }

        let to_send = cmp::min(self.data.len() - self.position, 0xFFE);
        self.bus.transfer_data(&self.data[self.position..(self.position + to_send)])?;
        self.position += to_send;

        if self.position != self.data.len() {
            Ok(ProgrammerState::InProgress(self.position))
        } else {
            Ok(ProgrammerState::Completed)
        }
    }
}

/// Generates a key from a seed for security access
fn generate_key(key: &str, parameter: u32, seed: &[u8]) -> [u8; 3] {
    let mut parameter = parameter;
    // This is Mazda's key generation algorithm reverse engineered from a
    // Mazda 6 MPS ROM. Internally, the ECU uses a timer/counter for the seed
    // generation

    let nseed = {
        let mut nseed = seed.to_vec();
        nseed.extend_from_slice(key.as_bytes());
        nseed
    };

    for c in nseed.iter().cloned() {
        let mut c = c;
        for _ in (1..=8).rev() {
            let s = (c & 1) ^ (parameter & 1) as u8;
            let mut m: u32 = 0;
            if s != 0 {
                parameter |= 0x0100_0000;
                m = 0x0010_9028;
            }

            c >>= 1;
            parameter >>= 1;
            let p3 = parameter & 0xFFEF_6FD7;
            parameter ^= m;
            parameter &= 0x0010_9028;

            parameter |= p3;
            parameter &= 0x00FF_FFFF;
        }
    }

    let mut res = [0; 3];
    res[0] = ((parameter >> 4) & 0xFF) as u8;
    res[1] = (((parameter >> 20) & 0xFF) + ((parameter >> 8) & 0xF0)) as u8;
    res[2] = (((parameter << 4) & 0xFF) + ((parameter >> 16) & 0x0F)) as u8;

    res
}
