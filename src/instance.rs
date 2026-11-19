use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, net::UnixDatagram},
    },
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Toggle,
    Expand,
    Close,
}

impl Command {
    fn wire(self) -> u8 {
        match self {
            Self::Toggle => b't',
            Self::Expand => b'e',
            Self::Close => b'c',
        }
    }
    fn decode(data: &[u8]) -> Option<Self> {
        match data {
            [b't'] => Some(Self::Toggle),
            [b'e'] => Some(Self::Expand),
            [b'c'] => Some(Self::Close),
            _ => None,
        }
    }
}

pub struct Instance {
    _lock: File,
    path: PathBuf,
    socket: UnixDatagram,
}

impl Instance {
    pub fn acquire(dir: &Path, name: &str, command: Command) -> io::Result<Option<Self>> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join(format!("noctalia-touch-{name}.lock")))?;
        let path = dir.join(format!("noctalia-touch-{name}.sock"));
        // The lock outlives the bound socket path, including during Drop.
        let mut last_error = None;
        for _ in 0..25 {
            // SAFETY: lock is an open descriptor owned by this function.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                match fs::remove_file(&path) {
                    Ok(()) => (),
                    Err(err) if err.kind() == io::ErrorKind::NotFound => (),
                    Err(err) => return Err(err),
                }
                let socket = UnixDatagram::bind(&path)?;
                return Ok(Some(Self {
                    _lock: lock,
                    path,
                    socket,
                }));
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(err);
            }
            let client = UnixDatagram::unbound()?;
            client.set_nonblocking(true)?;
            match client.send_to(&[command.wire()], &path) {
                Ok(_) => return Ok(None),
                Err(err) => last_error = Some(err),
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("existing instance not responding")))
    }

    pub fn listen(&self) -> io::Result<async_channel::Receiver<Command>> {
        let socket = self.socket.try_clone()?;
        let (send, recv) = async_channel::bounded(8);
        thread::spawn(move || {
            let mut buf = [0; 4];
            loop {
                match socket.recv(&mut buf) {
                    Ok(n) => {
                        if let Some(command) = Command::decode(&buf[..n])
                            && send.send_blocking(command).is_err()
                        {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        Ok(recv)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_invocation_notifies_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Instance::acquire(dir.path(), "keyboard", Command::Expand)
            .unwrap()
            .unwrap();
        assert!(
            Instance::acquire(dir.path(), "keyboard", Command::Toggle)
                .unwrap()
                .is_none()
        );
        let mut data = [0; 4];
        owner
            .socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let n = owner.socket.recv(&mut data).unwrap();
        assert_eq!(Command::decode(&data[..n]), Some(Command::Toggle));
        drop(owner);
        assert!(
            Instance::acquire(dir.path(), "keyboard", Command::Expand)
                .unwrap()
                .is_some()
        );
    }
}
