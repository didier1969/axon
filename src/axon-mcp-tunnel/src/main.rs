use tokio::net::UnixStream;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use std::process;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket_path = "/tmp/axon-mcp.sock";
    
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Error: Could not connect to Axon MCP socket at {}. Is Axon Core running?", socket_path);
            process::exit(1);
        }
    };

    let (mut sock_reader, mut sock_writer) = stream.into_split();
    
    // stdin -> socket
    tokio::spawn(async move {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if sock_writer.write_all(line.as_bytes()).await.is_err() { break; }
            line.clear();
        }
    });

    // socket -> stdout
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout);
    let mut reader = BufReader::new(sock_reader);
    let mut line = String::new();
    while let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 { break; }
        writer.write_all(line.as_bytes()).await?;
        writer.flush().await?;
        line.clear();
    }

    Ok(())
}
