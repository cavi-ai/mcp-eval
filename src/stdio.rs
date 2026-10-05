#[cfg(unix)]
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::net::Shutdown;
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(any(test, windows))]
use std::thread;
use std::thread::JoinHandle;
#[cfg(windows)]
use windows_sys::Win32::{Foundation::ERROR_NOT_FOUND, System::IO::CancelSynchronousIo};

#[cfg(any(test, windows))]
pub(crate) fn cancel_synchronous_io_until_observed(
    mut is_finished: impl FnMut() -> bool,
    mut cancel_once: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    loop {
        if is_finished() || cancel_once()? {
            return Ok(());
        }
        thread::yield_now();
    }
}

#[cfg(unix)]
pub(crate) struct CancelHandle {
    sender: UnixStream,
}

#[cfg(unix)]
impl CancelHandle {
    pub(crate) fn cancel(&self) -> io::Result<()> {
        match self.sender.shutdown(Shutdown::Write) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotConnected => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// Pipe writes must be interruptible even when the child stops reading.
pub(crate) struct CancellableWriter<R>(CancellableReader<R>);

#[cfg(unix)]
impl<R: AsFd> CancellableWriter<R> {
    pub(crate) fn new(source: R, cancellation: UnixStream) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let fd = source.as_fd().as_raw_fd();
        // SAFETY: source owns fd throughout these calls; the flags only affect this pipe.
        let flags = unsafe { nix::libc::fcntl(fd, nix::libc::F_GETFL) };
        if flags == -1
            || unsafe { nix::libc::fcntl(fd, nix::libc::F_SETFL, flags | nix::libc::O_NONBLOCK) }
                == -1
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(CancellableReader::new(source, cancellation)))
    }
}

#[cfg(windows)]
impl<R> CancellableWriter<R> {
    pub(crate) fn new(source: R, cancellation: Arc<AtomicBool>) -> io::Result<Self> {
        Ok(Self(CancellableReader::new(source, cancellation)))
    }
}

#[cfg(unix)]
impl<R: Write + AsFd> Write for CancellableWriter<R> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            let mut descriptors = [
                PollFd::new(
                    self.0.cancellation.as_fd(),
                    PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                ),
                PollFd::new(
                    self.0.source.as_fd(),
                    PollFlags::POLLOUT | PollFlags::POLLHUP | PollFlags::POLLERR,
                ),
            ];
            match poll(&mut descriptors, PollTimeout::NONE) {
                Err(nix::errno::Errno::EINTR) => continue,
                Err(error) => return Err(io::Error::from_raw_os_error(error as i32)),
                Ok(_) => {}
            }
            if !descriptors[0]
                .revents()
                .unwrap_or_else(PollFlags::empty)
                .is_empty()
            {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "stdio pump cancelled",
                ));
            }
            match self.0.source.write(buffer) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.source.flush()
    }
}

#[cfg(windows)]
impl<R: Write> Write for CancellableWriter<R> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.0.is_cancelled() {
            return Err(cancelled_io_error());
        }
        let result = self.0.source.write(buffer);
        if self.0.is_cancelled() {
            return Err(cancelled_io_error());
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.source.flush()
    }
}

#[cfg(unix)]
pub(crate) fn cancellation_pair() -> io::Result<(CancelHandle, UnixStream)> {
    let (sender, receiver) = UnixStream::pair()?;
    Ok((CancelHandle { sender }, receiver))
}

#[cfg(unix)]
pub(crate) struct CancellableReader<R> {
    source: R,
    cancellation: UnixStream,
    cancelled: bool,
}

#[cfg(unix)]
impl<R> CancellableReader<R> {
    pub(crate) fn new(source: R, cancellation: UnixStream) -> Self {
        Self {
            source,
            cancellation,
            cancelled: false,
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

#[cfg(unix)]
impl<R: Read + AsFd> Read for CancellableReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let (source_events, cancellation_events) = {
                let mut descriptors = [
                    PollFd::new(
                        self.cancellation.as_fd(),
                        PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                    ),
                    PollFd::new(
                        self.source.as_fd(),
                        PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                    ),
                ];
                match poll(&mut descriptors, PollTimeout::NONE) {
                    Ok(_) => (
                        descriptors[1].revents().unwrap_or_else(PollFlags::empty),
                        descriptors[0].revents().unwrap_or_else(PollFlags::empty),
                    ),
                    Err(nix::errno::Errno::EINTR) => continue,
                    Err(error) => return Err(io::Error::from_raw_os_error(error as i32)),
                }
            };

            if cancellation_events.intersects(
                PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
            ) {
                self.cancelled = true;
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "stdio pump cancelled",
                ));
            }
            if source_events.intersects(
                PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
            ) {
                return self.source.read(buffer);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) struct CancelHandle {
    cancelled: Arc<AtomicBool>,
}

#[cfg(windows)]
pub(crate) fn cancellation_pair() -> io::Result<(CancelHandle, Arc<AtomicBool>)> {
    let cancelled = Arc::new(AtomicBool::new(false));
    Ok((
        CancelHandle {
            cancelled: Arc::clone(&cancelled),
        },
        cancelled,
    ))
}

#[cfg(windows)]
pub(crate) struct CancellableReader<R> {
    source: R,
    cancellation: Arc<AtomicBool>,
}

#[cfg(windows)]
impl<R> CancellableReader<R> {
    pub(crate) fn new(source: R, cancellation: Arc<AtomicBool>) -> Self {
        Self {
            source,
            cancellation,
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }
}

#[cfg(windows)]
impl<R: Read> Read for CancellableReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.is_cancelled() {
            return Err(cancelled_io_error());
        }
        let result = self.source.read(buffer);
        if self.is_cancelled() {
            return Err(cancelled_io_error());
        }
        result
    }
}

#[cfg(windows)]
fn cancelled_io_error() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "stdio pump cancelled")
}

impl CancelHandle {
    pub(crate) fn cancel_thread<T>(&self, _thread: &JoinHandle<T>) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.cancel()
        }
        #[cfg(windows)]
        {
            self.cancelled.store(true, Ordering::Release);
            cancel_synchronous_io_until_observed(
                || _thread.is_finished(),
                || {
                    // SAFETY: JoinHandle owns this valid thread handle for the
                    // duration of the call, and the API only borrows it.
                    let cancelled = unsafe { CancelSynchronousIo(_thread.as_raw_handle() as _) };
                    if cancelled != 0 {
                        return Ok(true);
                    }
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                        Ok(false)
                    } else {
                        Err(error)
                    }
                },
            )
        }
    }
}
