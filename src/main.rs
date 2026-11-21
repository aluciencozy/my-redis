use resp::{HandlerError, RespHandler, RespValue};
use tokio::net::{TcpListener, TcpStream};

mod resp;

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();

    loop {
        let (socket, _) = listener.accept().await.unwrap();

        tokio::spawn(async move {
            if let Err(err) = handle_connection(socket).await {
                eprintln!("connection error: {err}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream) -> Result<(), HandlerError> {
    let mut handler = RespHandler::new(stream);

    while let Some(request) = handler.read_value().await? {
        if let Some(reply) = command_reply(request) {
            handler.write_reply(&reply).await?;
        }
    }

    Ok(())
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
