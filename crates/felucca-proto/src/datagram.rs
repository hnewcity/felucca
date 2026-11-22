//! Datagram boundaries over a reliable tunnel stream, including empty packets.
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_DATAGRAM: usize = 65_507;

pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let len = reader.read_u16().await? as usize;
    if len > MAX_DATAGRAM {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized UDP datagram",
        ));
    }
    let mut data = vec![0; len];
    reader.read_exact(&mut data).await?;
    Ok(data)
}

pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, packet: &[u8]) -> io::Result<()> {
    if packet.len() > MAX_DATAGRAM {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "oversized UDP datagram",
        ));
    }
    writer.write_u16(packet.len() as u16).await?;
    writer.write_all(packet).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_and_maximum_packets_remain_distinct_across_partial_reads() {
        let (mut send, mut receive) = tokio::io::duplex(7);
        let writer = tokio::spawn(async move {
            write(&mut send, b"").await.unwrap();
            write(&mut send, &vec![42; MAX_DATAGRAM]).await.unwrap();
            write(&mut send, b"last").await.unwrap();
        });
        assert!(read(&mut receive).await.unwrap().is_empty());
        assert_eq!(read(&mut receive).await.unwrap(), vec![42; MAX_DATAGRAM]);
        assert_eq!(read(&mut receive).await.unwrap(), b"last");
        writer.await.unwrap();
        assert_eq!(
            read(&mut receive).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test]
    async fn malformed_datagrams_are_rejected_without_losing_boundaries() {
        let mut sink = Vec::new();
        assert_eq!(
            write(&mut sink, &vec![0; MAX_DATAGRAM + 1])
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(sink.is_empty());
        let mut oversized: &[u8] = &[255, 255];
        assert_eq!(
            read(&mut oversized).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut truncated: &[u8] = &[0, 3, 1, 2];
        assert_eq!(
            read(&mut truncated).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
