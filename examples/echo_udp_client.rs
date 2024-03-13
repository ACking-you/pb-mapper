use std::error::Error;

use pb_mapper::common::config::init_tracing;
use pb_mapper::utils::udp::UdpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();

    let mut stream = UdpStream::connect("127.0.0.1:8080").await?;
    tracing::info!("Ready to Connected to {}", &stream.peer_addr()?);
    let mut buffer = String::new();
    loop {
        std::io::stdin().read_line(&mut buffer)?;
        stream.write_all(buffer.as_bytes()).await?;
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await?;
        tracing::info!("-> {}", String::from_utf8_lossy(&buf[..n]));
        buffer.clear();
    }
}
