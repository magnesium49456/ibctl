//! Minimal protocol-level IB API readiness probe.
//!
//! A TCP accept alone is insufficient: Gateway can leave a listener behind
//! while the API session is unusable.  This probe performs the documented
//! v100 handshake and requires a plausible server-version response.

use std::io;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn probe(host: &str, port: u16, timeout: Duration) -> io::Result<()> {
    tokio::time::timeout(timeout, probe_inner(host, port))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "IB API handshake timed out"))?
}

async fn probe_inner(host: &str, port: u16) -> io::Result<()> {
    let mut stream = TcpStream::connect((host, port)).await?;
    stream.write_all(b"API\0").await?;
    let payload = b"v100..178\0";
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    stream.write_all(payload).await?;
    stream.flush().await?;

    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IB API handshake length",
        ));
    }
    let mut response = vec![0_u8; length];
    stream.read_exact(&mut response).await?;
    let fields = response
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let version = fields
        .first()
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse::<u32>().ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing IB API server version")
        })?;
    if version < 100 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "implausible IB API server version",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn accepts_real_protocol_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut preamble = [0_u8; 4];
            socket.read_exact(&mut preamble).await.unwrap();
            assert_eq!(&preamble, b"API\0");
            let mut len = [0_u8; 4];
            socket.read_exact(&mut len).await.unwrap();
            let mut request = vec![0; u32::from_be_bytes(len) as usize];
            socket.read_exact(&mut request).await.unwrap();
            let response = b"178\020260812 16:00:00 EST\0";
            socket
                .write_all(&(response.len() as u32).to_be_bytes())
                .await
                .unwrap();
            socket.write_all(response).await.unwrap();
        });
        probe("127.0.0.1", port, Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_plain_tcp_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        assert!(probe("127.0.0.1", port, Duration::from_millis(100))
            .await
            .is_err());
    }
}
