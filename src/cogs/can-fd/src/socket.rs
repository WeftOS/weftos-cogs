//! A receive-only raw SocketCAN socket (Linux).
//!
//! It binds to one interface, asks for CAN FD frames and for error frames, and reads with a
//! timeout. Nothing in this cog writes to the socket: transmitting on a bus is a different
//! decision (WeftOS ADR-123, not accepted) and needs a grant this cog does not have.

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Duration;

    pub struct Can {
        fd: OwnedFd,
        #[allow(dead_code)]
        pub fd_enabled: bool,
    }

    fn last() -> String {
        std::io::Error::last_os_error().to_string()
    }

    impl Can {
        pub fn open(iface: &str) -> Result<Self, String> {
            let name = std::ffi::CString::new(iface).map_err(|_| "bad interface name")?;
            // SAFETY: plain syscalls; every result is checked, and the descriptor is owned at once.
            unsafe {
                let ifindex = libc::if_nametoindex(name.as_ptr());
                if ifindex == 0 {
                    return Err(format!("no interface {iface:?}"));
                }
                let s = libc::socket(libc::PF_CAN, libc::SOCK_RAW, libc::CAN_RAW);
                if s < 0 {
                    return Err(format!(
                        "socket(PF_CAN, SOCK_RAW, CAN_RAW): {} (is the can_raw module loaded?)",
                        last()
                    ));
                }
                let fd = OwnedFd::from_raw_fd(s);
                let on: libc::c_int = 1;
                let fd_enabled = libc::setsockopt(
                    s,
                    libc::SOL_CAN_RAW,
                    libc::CAN_RAW_FD_FRAMES,
                    (&on as *const libc::c_int).cast(),
                    4,
                ) == 0;
                let mask: libc::can_err_mask_t = libc::CAN_ERR_MASK;
                if libc::setsockopt(
                    s,
                    libc::SOL_CAN_RAW,
                    libc::CAN_RAW_ERR_FILTER,
                    (&mask as *const libc::can_err_mask_t).cast(),
                    std::mem::size_of::<libc::can_err_mask_t>() as u32,
                ) != 0
                {
                    return Err(format!("CAN_RAW_ERR_FILTER: {}", last()));
                }
                let tv = libc::timeval {
                    tv_sec: 0,
                    tv_usec: 100_000,
                };
                libc::setsockopt(
                    s,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    (&tv as *const libc::timeval).cast(),
                    std::mem::size_of::<libc::timeval>() as u32,
                );
                let mut addr: libc::sockaddr_can = std::mem::zeroed();
                addr.can_family = libc::AF_CAN as libc::sa_family_t;
                addr.can_ifindex = ifindex as libc::c_int;
                if libc::bind(
                    s,
                    (&addr as *const libc::sockaddr_can).cast(),
                    std::mem::size_of::<libc::sockaddr_can>() as u32,
                ) != 0
                {
                    return Err(format!("bind {iface}: {}", last()));
                }
                Ok(Self { fd, fd_enabled })
            }
        }

        /// One frame, or `Ok(None)` when nothing arrived within the timeout.
        pub fn read(&self, wait_hint: Duration) -> Result<Option<Vec<u8>>, String> {
            let _ = wait_hint;
            let mut buf = [0u8; super::super::frame::FD_LEN];
            // SAFETY: `buf` is a live buffer of the length passed.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                return match e.kind() {
                    std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted => Ok(None),
                    _ => Err(format!("read: {e}")),
                };
            }
            Ok(Some(buf[..n as usize].to_vec()))
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::Can;

#[cfg(not(target_os = "linux"))]
pub struct Can;

#[cfg(not(target_os = "linux"))]
impl Can {
    pub fn open(_: &str) -> Result<Self, String> {
        Err("SocketCAN is Linux only".into())
    }

    pub fn read(&self, _: std::time::Duration) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
