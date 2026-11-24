use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use resp::{HandlerError, RespHandler, RespValue};
use tokio::net::{TcpListener, TcpStream};

mod resp;

struct Entry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

type Store = Arc<Mutex<HashMap<Vec<u8>, Entry>>>;

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

    const COMMAND_ERROR: &[u8] = b"-ERR unsupported command or wrong number of arguments\r\n";

    let Array(items) = request else {
        return None;
    };

    let mut args = items.into_iter();
    let Some(BulkString(command)) = args.next() else {
        return Some(COMMAND_ERROR.to_vec());
    };

    let reply = match command.as_slice() {
        command if command.eq_ignore_ascii_case(b"SET") => {
            let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next())
            else {
                return Some(COMMAND_ERROR.to_vec());
            };

            let expiry_bytes = match args.next() {
                None => None,
                Some(BulkString(option)) if option.eq_ignore_ascii_case(b"PX") => {
                    let (Some(BulkString(millis)), None) = (args.next(), args.next()) else {
                        return Some(COMMAND_ERROR.to_vec());
                    };
                    Some(millis)
                }
                _ => return Some(COMMAND_ERROR.to_vec()),
            };

            let expires_at = match expiry_bytes {
                None => None,
                Some(bytes) => {
                    let deadline = std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|text| text.parse::<u64>().ok())
                        .filter(|&millis| millis > 0)
                        .and_then(|millis| {
                            Instant::now().checked_add(Duration::from_millis(millis))
                        });

                    let Some(deadline) = deadline else {
                        return Some(b"-ERR invalid expire time in 'set' command\r\n".to_vec());
                    };

                    Some(deadline)
                }
            };

            store
                .lock()
                .unwrap()
                .insert(key, Entry { value, expires_at });
            b"+OK\r\n".to_vec()
        }
        command if command.eq_ignore_ascii_case(b"GET") => {
            let (Some(BulkString(key)), None) = (args.next(), args.next()) else {
                return Some(COMMAND_ERROR.to_vec());
            };

            let mut store = store.lock().unwrap();
            match store.get(&key) {
                Some(entry)
                    if entry
                        .expires_at
                        .is_some_and(|expiry| Instant::now() >= expiry) =>
                {
                    store.remove(&key);
                    b"$-1\r\n".to_vec()
                }
                Some(entry) => encode_bulk_string(&entry.value),
                None => b"$-1\r\n".to_vec(),
            }
        }
        command if command.eq_ignore_ascii_case(b"ECHO") => {
            let (Some(BulkString(argument)), None) = (args.next(), args.next()) else {
                return Some(COMMAND_ERROR.to_vec());
            };

            encode_bulk_string(&argument)
        }
        command if command.eq_ignore_ascii_case(b"PING") && args.next().is_none() => {
            b"+PONG\r\n".to_vec()
        }
        _ => COMMAND_ERROR.to_vec(),
    };

    Some(reply)
}

fn encode_bulk_string(bytes: &[u8]) -> Vec<u8> {
    let mut bulk_string = format!("${}\r\n", bytes.len()).into_bytes();
    bulk_string.extend_from_slice(bytes);
    bulk_string.extend_from_slice(b"\r\n");
    bulk_string
}
