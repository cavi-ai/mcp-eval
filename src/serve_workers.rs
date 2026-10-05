//! Bounded connection handling for the loopback MCP service.
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::Context;

const WORKERS: usize = 4;
const QUEUED_CONNECTIONS: usize = 16;

struct WorkerPool {
    sender: Option<mpsc::SyncSender<TcpStream>>,
    threads: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    fn new(handler: impl Fn(&mut TcpStream) + Send + Sync + 'static) -> anyhow::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<TcpStream>(QUEUED_CONNECTIONS);
        let receiver = Arc::new(Mutex::new(receiver));
        let handler = Arc::new(handler);
        let mut pool = Self {
            sender: Some(sender),
            threads: Vec::new(),
        };
        for index in 0..WORKERS {
            let receiver = Arc::clone(&receiver);
            let handler = Arc::clone(&handler);
            pool.threads.push(
                thread::Builder::new()
                    .name(format!("mcp-http-{index}"))
                    .spawn(move || loop {
                        // Hold the receiver lock only while taking a connection,
                        // never while processing it or running an evaluation.
                        let connection = match receiver.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => break,
                        };
                        let Ok(mut stream) = connection else { break };
                        handler(&mut stream);
                    })
                    .context("starting MCP HTTP worker")?,
            );
        }
        Ok(pool)
    }

    /// Shed excess connections immediately; never queue unbounded sockets
    /// or create one thread per incoming connection.
    fn submit(&self, stream: TcpStream) -> anyhow::Result<bool> {
        match self
            .sender
            .as_ref()
            .context("MCP HTTP workers stopped")?
            .try_send(stream)
        {
            Ok(()) => Ok(true),
            Err(mpsc::TrySendError::Full(_)) => Ok(false),
            Err(mpsc::TrySendError::Disconnected(_)) => anyhow::bail!("MCP HTTP workers stopped"),
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.sender.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

pub(crate) fn run(
    listener: TcpListener,
    handler: impl Fn(&mut TcpStream) + Send + Sync + 'static,
) -> anyhow::Result<()> {
    let pool = WorkerPool::new(handler)?;
    for connection in listener.incoming() {
        pool.submit(connection.context("accepting MCP connection")?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::Condvar;
    use std::time::Duration;

    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            let (lock, wake) = &*self.0;
            *lock.lock().unwrap() = true;
            wake.notify_all();
        }
    }

    fn connection(listener: &TcpListener) -> (TcpStream, TcpStream) {
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        (client, listener.accept().unwrap().0)
    }

    #[test]
    fn a_full_queue_closes_excess_connections_and_shutdown_joins_workers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = Arc::clone(&release);
        let (started, observed) = mpsc::channel();
        let pool = WorkerPool::new(move |_| {
            started.send(()).unwrap();
            let (lock, wake) = &*gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
        })
        .unwrap();
        // Release blocked callbacks even if an assertion panics, before
        // pool drop joins them.
        let cleanup = Release(release);
        let mut clients = Vec::new();
        for _ in 0..WORKERS {
            let (client, server) = connection(&listener);
            assert!(pool.submit(server).unwrap());
            clients.push(client);
            observed.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for _ in 0..QUEUED_CONNECTIONS {
            let (client, server) = connection(&listener);
            assert!(pool.submit(server).unwrap());
            clients.push(client);
        }
        let (mut excess, server) = connection(&listener);
        assert!(!pool.submit(server).unwrap());
        excess
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(excess.read(&mut [0]).unwrap(), 0);
        drop(cleanup);
        drop(pool);
        for mut client in clients {
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            assert_eq!(client.read(&mut [0]).unwrap(), 0);
        }
    }
}
