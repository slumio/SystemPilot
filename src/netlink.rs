//! Audited Linux process-connector boundary.
//!
//! Invariants: every descriptor is uniquely owned; syscall pointers refer to
//! live, correctly sized buffers; received lengths are checked before slicing;
//! and no kernel-provided length is trusted without bounds validation.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Fork { parent: i32, child: i32 },
    Exec { pid: i32 },
    Exit { pid: i32, status: i32, signal: i32 },
    KernelError(i32),
}

pub struct ProcessConnector(OwnedFd);

impl ProcessConnector {
    pub fn open() -> io::Result<Self> {
        // SAFETY: constant Linux netlink arguments; a successful descriptor has no owner yet.
        let raw = unsafe {
            libc::socket(
                libc::PF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                libc::NETLINK_CONNECTOR,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is nonnegative, newly returned, and uniquely owned.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: zero is the kernel-defined initialization for sockaddr_nl padding.
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as u16;
        address.nl_groups = 1;
        address.nl_pid = std::process::id();
        // SAFETY: the address pointer is live and its exact size is supplied.
        if unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &address as *const _ as *const libc::sockaddr,
                std::mem::size_of_val(&address) as libc::socklen_t,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let subscription = subscription_message(std::process::id());
        // SAFETY: zero is the kernel-defined initialization for sockaddr_nl padding.
        let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        kernel.nl_family = libc::AF_NETLINK as u16;
        // SAFETY: both pointers are live for the duration and exact lengths are supplied.
        let sent = unsafe {
            libc::sendto(
                fd.as_raw_fd(),
                subscription.as_ptr().cast(),
                subscription.len(),
                0,
                &kernel as *const _ as *const libc::sockaddr,
                std::mem::size_of_val(&kernel) as libc::socklen_t,
            )
        };
        if sent != subscription.len() as isize {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(fd))
    }

    pub fn receive(&self, buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the mutable buffer is valid for `buffer.len()` bytes.
        let length = unsafe {
            libc::recv(
                self.0.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        if length < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(length as usize)
        }
    }
}

fn subscription_message(pid: u32) -> [u8; 40] {
    let mut bytes = [0; 40];
    bytes[0..4].copy_from_slice(&40u32.to_ne_bytes());
    bytes[4..6].copy_from_slice(&3u16.to_ne_bytes());
    bytes[12..16].copy_from_slice(&pid.to_ne_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_ne_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_ne_bytes());
    bytes[28..30].copy_from_slice(&4u16.to_ne_bytes());
    bytes[36..40].copy_from_slice(&1u32.to_ne_bytes());
    bytes
}

pub fn parse_frames(bytes: &[u8]) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut offset = 0usize;
    while let Some(header) = bytes.get(offset..offset.saturating_add(16)) {
        let length = u32::from_ne_bytes(header[0..4].try_into().expect("checked header")) as usize;
        if length < 16
            || offset
                .checked_add(length)
                .is_none_or(|end| end > bytes.len())
        {
            break;
        }
        let kind = u16::from_ne_bytes(header[4..6].try_into().expect("checked header"));
        if kind == 2 {
            let code = bytes
                .get(offset + 16..offset + 20)
                .map(|value| i32::from_ne_bytes(value.try_into().expect("checked error")))
                .unwrap_or(-libc::EPROTO);
            if code != 0 {
                frames.push(Frame::KernelError(code));
            }
        } else if let Some(connector) = bytes.get(offset + 16..offset + length) {
            parse_connector(connector, &mut frames);
        }
        let aligned = length.checked_add(3).map(|value| value & !3).unwrap_or(0);
        if aligned < 16 {
            break;
        }
        offset = match offset.checked_add(aligned) {
            Some(next) => next,
            None => break,
        };
    }
    frames
}

fn parse_connector(bytes: &[u8], frames: &mut Vec<Frame>) {
    if bytes.len() < 24 || read_u32(bytes, 0) != Some(1) || read_u32(bytes, 4) != Some(1) {
        return;
    }
    let event = &bytes[20..];
    match read_u32(event, 0) {
        Some(1) if event.len() >= 32 => frames.push(Frame::Fork {
            parent: read_i32(event, 16).expect("checked fork"),
            child: read_i32(event, 24).expect("checked fork"),
        }),
        Some(2) if event.len() >= 24 => frames.push(Frame::Exec {
            pid: read_i32(event, 16).expect("checked exec"),
        }),
        Some(0x8000_0000) if event.len() >= 32 => frames.push(Frame::Exit {
            pid: read_i32(event, 16).expect("checked exit"),
            status: read_i32(event, 24).expect("checked exit"),
            signal: read_i32(event, 28).expect("checked exit"),
        }),
        _ => {}
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_short_and_invalid_lengths() {
        assert!(parse_frames(&[]).is_empty());
        assert!(parse_frames(&[0; 15]).is_empty());
        let mut invalid = [0; 16];
        invalid[0..4].copy_from_slice(&15u32.to_ne_bytes());
        assert!(parse_frames(&invalid).is_empty());
        invalid[0..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(parse_frames(&invalid).is_empty());
    }

    #[test]
    fn parser_surfaces_kernel_errors() {
        let mut bytes = [0; 20];
        bytes[0..4].copy_from_slice(&20u32.to_ne_bytes());
        bytes[4..6].copy_from_slice(&2u16.to_ne_bytes());
        bytes[16..20].copy_from_slice(&(-libc::EPERM).to_ne_bytes());
        assert_eq!(parse_frames(&bytes), vec![Frame::KernelError(-libc::EPERM)]);
    }
}
