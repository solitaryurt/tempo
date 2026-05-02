//! Single-instance handoff for repeated launches.
//!
//! The first Tempo process binds a small Unix-domain socket in the
//! user's runtime directory. Later launches connect to that socket,
//! send a focus command, and exit before GPUI starts. This keeps one
//! playback/catalog process alive while making desktop launcher clicks
//! behave like "show the existing window".

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::{
        fs,
        io::{self, Write},
        os::unix::net::{UnixListener, UnixStream},
        path::PathBuf,
    };

    const SOCKET_NAME: &str = "tempo-single-instance.sock";
    const FOCUS_COMMAND: &[u8] = b"focus\n";

    pub struct SingleInstanceServer {
        listener: UnixListener,
        path: PathBuf,
    }

    impl SingleInstanceServer {
        pub fn bind() -> io::Result<Self> {
            let path = socket_path();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }

            match UnixListener::bind(&path) {
                Ok(listener) => {
                    listener.set_nonblocking(true)?;
                    Ok(Self { listener, path })
                }
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    if send_focus_request().is_ok() {
                        return Err(error);
                    }
                    let _ = fs::remove_file(&path);
                    let listener = UnixListener::bind(&path)?;
                    listener.set_nonblocking(true)?;
                    Ok(Self { listener, path })
                }
                Err(error) => Err(error),
            }
        }

        pub fn try_accept_focus_request(&self) -> io::Result<bool> {
            match self.listener.accept() {
                Ok((_stream, _addr)) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
                Err(error) => Err(error),
            }
        }
    }

    impl Drop for SingleInstanceServer {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    pub fn send_focus_request() -> io::Result<()> {
        let mut stream = UnixStream::connect(socket_path())?;
        stream.write_all(FOCUS_COMMAND)
    }

    fn socket_path() -> PathBuf {
        if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(runtime_dir).join(SOCKET_NAME);
        }
        std::env::temp_dir().join(format!("tempo-{}-{SOCKET_NAME}", user_id()))
    }

    fn user_id() -> String {
        std::env::var("UID")
            .ok()
            .filter(|uid| !uid.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub use imp::{SingleInstanceServer, send_focus_request};

#[cfg(not(all(unix, not(target_os = "macos"))))]
mod imp {
    use std::io;

    pub struct SingleInstanceServer;

    impl SingleInstanceServer {
        pub fn bind() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn try_accept_focus_request(&self) -> io::Result<bool> {
            Ok(false)
        }
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub use imp::SingleInstanceServer;
