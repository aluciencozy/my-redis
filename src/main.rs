use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use resp::{HandlerError, RespHandler, RespValue};
use tokio::net::{TcpListener, TcpStream};

mod resp;

type Store = Arc<Mutex<HashMap<Vec<u8>, Vec<u8>>>>;

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();
    let store: Store = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let store = Arc::clone(&store);

        tokio::spawn(async move {
            if let Err(err) = handle_connection(socket, store).await {
                eprintln!("connection error: {err}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, store: Store) -> Result<(), HandlerError> {
    let mut handler = RespHandler::new(stream);

    while let Some(request) = handler.read_value().await? {
        if let Some(reply) = command_reply(request, &store) {
            handler.write_reply(&reply).await?;
        }
    }

    Ok(())
}

fn command_reply(request: RespValue, store: &Store) -> Option<Vec<u8>> {
    use RespValue::{Array, BulkString};

    let Array(items) = request else {
        return None;
    };

    let reply = match items.as_slice() {
        [BulkString(command), BulkString(key), BulkString(value)]
            if command.eq_ignore_ascii_case(b"SET") =>
        {
            let mut store = store.lock().unwrap();
            store.insert(key.clone(), value.clone());
            b"+OK\r\n".to_vec()
        }
        [BulkString(command), BulkString(key)] if command.eq_ignore_ascii_case(b"GET") => {
            let store = store.lock().unwrap();
            match store.get(key) {
                Some(value) => encode_bulk_string(value),
                None => b"$-1\r\n".to_vec(),
            }
        }
        [BulkString(command), BulkString(argument)] if command.eq_ignore_ascii_case(b"ECHO") => {
            encode_bulk_string(argument)
        }
        [BulkString(command)] if command.eq_ignore_ascii_case(b"PING") => b"+PONG\r\n".to_vec(),
        _ => b"-ERR unsupported command or wrong number of arguments\r\n".to_vec(),
    };

    Some(reply)
}

fn encode_bulk_string(bytes: &[u8]) -> Vec<u8> {
    let mut bulk_string = format!("${}\r\n", bytes.len()).into_bytes();
    bulk_string.extend_from_slice(bytes);
    bulk_string.extend_from_slice(b"\r\n");
    bulk_string
}
