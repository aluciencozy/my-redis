use resp::RespValue;
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
            let (request, next_pos) = match resp::parse_resp(&pending, 0) {
                Ok(Some(parsed)) => parsed,
                Ok(None) => break,
                Err(err) => {
                    eprintln!("invalid request: {err}");
                    return;
                }
            };

            if let Some(reply) = command_reply(request) {
                stream.write_all(&reply).await.unwrap();
            }

            pending.drain(..next_pos);
        }
    }
}

fn command_reply(request: RespValue) -> Option<Vec<u8>> {
    use RespValue::{Array, BulkString};

    let Array(items) = request else {
        return None;
    };

    let reply = match items.as_slice() {
        [BulkString(command), BulkString(argument)] if command.eq_ignore_ascii_case(b"ECHO") => {
            let mut reply = format!("${}\r\n", argument.len()).into_bytes();
            reply.extend_from_slice(argument);
            reply.extend_from_slice(b"\r\n");
            reply
        }
        [BulkString(command)] if command.eq_ignore_ascii_case(b"PING") => b"+PONG\r\n".to_vec(),
        _ => b"-ERR unsupported command or wrong number of arguments\r\n".to_vec(),
    };

    Some(reply)
}
