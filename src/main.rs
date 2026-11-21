#![allow(unused_imports)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

mod resp;

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();

    loop {
        let (socket, _) = listener.accept().await.unwrap();

        tokio::spawn(async move {
            handle_connection(socket).await;
        });
    }
}

async fn handle_connection(mut stream: TcpStream) {
    let mut chunk = [0u8; 512];
    let mut pending = Vec::new();

    loop {
        let bytes_read = stream.read(&mut chunk).await.unwrap();
        if bytes_read == 0 {
            break;
        }

        pending.extend_from_slice(&chunk[..bytes_read]);

        loop {
            match resp::parse_resp(&pending, 0) {
                Ok(Some((request, next_pos))) => {
                    println!("{request:?}");
                    pending.drain(..next_pos);
                }
                Ok(None) => break,
                Err(err) => {
                    eprintln!("invalid request: {err}");
                    return;
                }
            }
        }
    }
}
