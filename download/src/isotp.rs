use std::cmp;
use std::convert::TryFrom;
use std::default::Default;
use std::io;
use std::rc::Rc;
use std::result::Result;
use std::thread;
use std::time;
use std::time::{Duration, Instant};

#[cfg(feature = "socketcan-datalink")]
use socketcan::CANError;
use thiserror::Error;

use crate::datalink::can::{Can, Message};

#[derive(Error, Debug)]
pub enum IsotpError {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("invalid flow control flag")]
    InvalidFcFlag,

    #[error("invalid frame id")]
    InvalidFrameId,

    #[error("timed out")]
    TimedOut,

    /// Occurs when a frame is received with an unexpected id, e.g. when waiting for a
    /// flow control frame but another frame was received.
    #[error("unexpected frame")]
    UnexpectedFrame,

    #[error("invalid consecutive frame index")]
    InvalidIndex,
}

#[derive(Debug, Copy, Clone)]
pub enum FCFlag {
    Continue = 0,
    Wait = 1,
    Overflow = 2,
}

#[derive(Debug)]
pub enum Frame {
    Single {
        length: u8,
        data: [u8; 7],
    },
    First {
        size: u16,
        data: [u8; 6],
    },
    Consecutive {
        index: u8,
        data: [u8; 7],
    },
    Flow {
        flag: FCFlag,
        block_size: u8,
        separation_time: Duration,
    },
}

impl Frame {
    /// Creates a consecutive frame. `data` must be less than 8 bytes long.
    fn consecutive(data: &[u8], index: u8) -> Frame {
        assert!(data.len() <= 7);
        let mut frame_data = [0_u8; 7];
        frame_data[..data.len()].copy_from_slice(&data);
        Frame::Consecutive {
            index,
            data: frame_data,
        }
    }

    /// Creates a first frame. `data` must be less than 7 bytes long.
    fn first(data: &[u8], size: u16) -> Frame {
        assert!(data.len() <= 6);
        let mut frame_data = [0_u8; 6];
        frame_data[..data.len()].copy_from_slice(&data);
        Frame::First {
            size,
            data: frame_data,
        }
    }

    /// Creates a single frame. `data` must be less than 8 bytes long.
    fn single(data: &[u8]) -> Frame {
        assert!(data.len() <= 7);
        let mut frame_data = [0_u8; 7];
        frame_data[..data.len()].copy_from_slice(data);
        Frame::Single {
            length: data.len() as u8,
            data: frame_data,
        }
    }

    /// Encodes ISO-TP [`Frame`] to a CAN Message
    fn as_can_message(&self, id: u32) -> Message {
        let mut message_data = [0_u8; 8];
        match *self {
            Frame::Single { length, data } => {
                message_data[0] = length;
                message_data[1..8].copy_from_slice(&data);
            }
            Frame::First { size, data } => {
                message_data[0] = (1 << 4) | ((size & 0xF00) >> 16) as u8;
                message_data[1] = (size & 0xFF) as u8;
                message_data[2..8].copy_from_slice(&data);
            }
            Frame::Consecutive { index, data } => {
                message_data[0] = (2 << 4) | index;
                message_data[1..8].copy_from_slice(&data);
            }
            Frame::Flow {
                flag,
                block_size,
                separation_time,
            } => {
                message_data[0] = 0x30 | (flag as u8);
                message_data[2] = duration_to_st(separation_time);
                message_data[1] = block_size;
            }
        };
        Message {
            id,
            data: message_data,
            len: 8,
        }
    }
}

impl TryFrom<Message> for Frame {
    type Error = IsotpError;

    /// Converts from CAN message. Ignores message length. Returns Err(()) for invalid frames.
    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        let code = (msg.data[0] & 0xF0) >> 8;
        match code {
            0 => {
                // Single frame
                let mut data = [0_u8; 7];
                data.copy_from_slice(&msg.data[1..8]);
                let length = msg.data[0] & 0x07;
                Ok(Frame::Single { length, data })
            }
            1 => {
                // First
                let size = ((msg.data[0] as u16 & 0x0F) << 16) | msg.data[1] as u16;
                let mut data = [0_u8; 6];
                data.copy_from_slice(&msg.data[2..8]);
                Ok(Frame::First { size, data })
            }
            2 => {
                // Consecutive
                let index = msg.data[0] & 0x0F;
                let mut data = [0_u8; 7];
                data.copy_from_slice(&msg.data[1..8]);
                Ok(Frame::Consecutive { index, data })
            }
            3 => {
                // Flow
                let flag = match msg.data[0] & 0x03 {
                    0 => FCFlag::Continue,
                    1 => FCFlag::Wait,
                    2 => FCFlag::Overflow,
                    _ => return Err(IsotpError::InvalidFcFlag),
                };
                let block_size = msg.data[1];
                let separation_time = msg.data[2];
                Ok(Frame::Flow {
                    flag,
                    block_size,
                    separation_time: st_to_duration(separation_time),
                })
            }
            _ => Err(IsotpError::InvalidFrameId),
        }
    }
}

pub trait Isotp {
    /// Receives an ISO-TP packet
    fn read_isotp(&self) -> Result<Vec<u8>, IsotpError>;

    /// Sends an ISO-TP packet
    fn write_isotp(&self, data: &[u8]) -> Result<(), IsotpError>;

    fn request_isotp(&self, request: &[u8]) -> Result<Vec<u8>, IsotpError> {
        self.write_isotp(&request)?;
        self.read_isotp()
    }
}

/// Converts separation time to [`Duration`]
fn st_to_duration(st: u8) -> Duration {
    if st <= 127 {
        return Duration::from_millis(st as u64);
    }
    Duration::from_micros(st as u64)
}

/// Converts [`Duration`] to separation time
fn duration_to_st(duration: Duration) -> u8 {
    if duration.subsec_micros() <= 900 && duration.subsec_micros() >= 100 {
        return (cmp::max(duration.subsec_micros() / 100, 1) + 0xF0) as u8;
    }
    duration.subsec_micros() as u8
}

struct SendPacket<'a> {
    buffer: &'a [u8],
    index: u8,
}

/// Used for sending mutli-frame packets.
/// It is NOT used for single-frame packets.
impl<'a> SendPacket<'a> {
    fn new(buffer: &[u8]) -> SendPacket {
        assert!(buffer.len() <= 4095);
        SendPacket { buffer, index: 0 }
    }

    fn first_frame(&mut self) -> Frame {
        let len = cmp::min(self.buffer.len(), 6);
        let mut data = [0_u8; 6];
        data.copy_from_slice(&self.buffer[..len]);
        let frame = Frame::First {
            size: self.buffer.len() as u16,
            data,
        };
        self.buffer = &self.buffer[len..];
        self.index = 1;
        frame
    }

    fn next_consec_frame(&mut self) -> Frame {
        let len = cmp::min(self.buffer.len(), 7);
        let frame = Frame::consecutive(&self.buffer[..len], self.index);
        self.buffer = &self.buffer[len..];
        self.index += 1;
        if self.index == 16 {
            self.index = 0;
        }
        frame
    }

    fn eof(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// ISO-TP stack implemented in user-space. Timing is likely nonconforming.
pub struct IsotpCan<C: Can> {
    can: C,
    pub source_id: u32,
    pub dest_id: u32,
    pub timeout: Duration,
}

impl<C: Can> IsotpCan<C> {
    pub fn new(can: C, source_id: u32, dest_id: u32, timeout: Duration) -> IsotpCan<C> {
        IsotpCan {
            can,
            source_id,
            dest_id,
            timeout,
        }
    }

    fn send_frame(&self, frame: &Frame) -> Result<(), IsotpError> {
        self.can.send_msg(&frame.as_can_message(self.source_id))?;
        Ok(())
    }

    fn recv_frame(&self) -> Result<Frame, IsotpError> {
        let start_time = Instant::now();
        loop {
            let msg = self.can.read(self.timeout)?;
            if msg.id == self.dest_id {
                return Ok(Frame::try_from(msg)?);
            }
            if start_time.elapsed() >= self.timeout {
                return Err(IsotpError::TimedOut);
            }
        }
        Err(IsotpError::TimedOut)
    }

    /// Returns (flag, block_size, separation_time)
    fn recv_flow_control_frame(&self) -> Result<(FCFlag, u8, Duration), IsotpError> {
        let frame = self.recv_frame()?;
        match frame {
            Frame::Flow {
                flag,
                block_size,
                separation_time,
            } => Ok((flag, block_size, separation_time)),
            _ => Err(IsotpError::UnexpectedFrame),
        }
    }
}

impl<C: Can> Isotp for IsotpCan<C> {
    fn read_isotp(&self) -> Result<Vec<u8>, IsotpError> {
        // Receive first or single frame
        let frame = self.recv_frame()?;
        match frame {
            Frame::Single { length, data } => Ok(data[..cmp::min(length as usize, 7)].to_vec()),
            Frame::First { size, data } => {
                let mut buffer = data[..cmp::min(size as usize, 6)].to_vec();
                let mut remaining = size as usize - buffer.len();
                // Send the flow control frame
                self.send_frame(&Frame::Flow {
                    flag: FCFlag::Continue,
                    block_size: 0,
                    separation_time: st_to_duration(0),
                })?;

                // Wait for all consecutive packets
                let mut index = 1;
                while remaining > 0 {
                    let (msg_index, data) = match self.recv_frame()? {
                        Frame::Consecutive { index, data } => (index, data),
                        _ => return Err(IsotpError::UnexpectedFrame),
                    };
                    if msg_index != index {
                        // Invalid index
                        return Err(IsotpError::InvalidIndex);
                    }

                    let len = cmp::min(remaining, 7);
                    buffer.extend_from_slice(&data);
                    remaining -= len;

                    index += 1;
                    if index == 16 {
                        index = 0;
                    }
                }
                Ok(buffer)
            }
            _ => Err(IsotpError::UnexpectedFrame),
        }
    }

    fn write_isotp(&self, data: &[u8]) -> Result<(), IsotpError> {
        if data.len() <= 7 {
            // Send a single frame
            self.send_frame(&Frame::single(data))?;
        } else {
            let mut packet = SendPacket::new(&data);
            // Send a first frame
            self.send_frame(&packet.first_frame())?;
            // Get flow control and send consecutive frames

            let (mut flag, mut block_size, mut separation_time) = self.recv_flow_control_frame()?;
            while !packet.eof() {
                // Loop until the buffer is empty
                if separation_time != Duration::new(0, 0) {
                    thread::sleep(separation_time);
                }

                self.send_frame(&packet.next_consec_frame())?;

                if !packet.eof() && block_size > 0 {
                    block_size -= 1;
                    if block_size == 0 {
                        // Get the next flow control packet
                        let (f_flag, f_block_size, f_separation_time) =
                            self.recv_flow_control_frame()?;
                        flag = f_flag;
                        block_size = f_block_size;
                        separation_time = f_separation_time;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use socketcan::CANSocket;

    #[test]
    #[cfg(unix)]
    fn isotp() {
        let can = CANSocket::open("test")?;
        can.write_isotp_frame("test", 0x170);
    }
}