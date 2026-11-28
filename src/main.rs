use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use resp::{
    HandlerError, RespHandler, RespValue,
    RespValue::{Array, BulkString},
};
use tokio::net::{TcpListener, TcpStream};

mod resp;

enum Value {
    String(Vec<u8>),
    List(VecDeque<Vec<u8>>),
}

struct Entry {
    value: Value,
    expires_at: Option<Instant>,
}

type Store = Arc<Mutex<HashMap<Vec<u8>, Entry>>>;
type CommandArgs = std::vec::IntoIter<RespValue>;

const COMMAND_ERROR_MESSAGE: &[u8] = b"ERR unsupported command or wrong number of arguments";

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
            let bytes = reply.encode();
            handler.write_reply(&bytes).await?;
        }
    }

    Ok(())
}

fn command_reply(request: RespValue, store: &Store) -> Option<RespValue> {
    let Array(items) = request else {
        return None;
    };

    let mut args = items.into_iter();
    let Some(BulkString(command)) = args.next() else {
        return Some(RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec()));
    };

    let reply = match command.as_slice() {
        command if command.eq_ignore_ascii_case(b"LPOP") => lpop(args, store),
        command if command.eq_ignore_ascii_case(b"LLEN") => llen(args, store),
        command if command.eq_ignore_ascii_case(b"LPUSH") => lpush(args, store),
        command if command.eq_ignore_ascii_case(b"LRANGE") => lrange(args, store),
        command if command.eq_ignore_ascii_case(b"RPUSH") => rpush(args, store),
        command if command.eq_ignore_ascii_case(b"SET") => set(args, store),
        command if command.eq_ignore_ascii_case(b"GET") => get(args, store),
        command if command.eq_ignore_ascii_case(b"ECHO") => echo(args),
        command if command.eq_ignore_ascii_case(b"PING") => ping(args),
        _ => RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec()),
    };

    Some(reply)
}

fn lpop(mut args: CommandArgs, store: &Store) -> RespValue {
    let Some(BulkString(key)) = args.next() else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let count = match (args.next(), args.next()) {
        (None, None) => None,
        (Some(BulkString(count)), None) => {
            let Ok(count_text) = std::str::from_utf8(&count) else {
                return RespValue::Error(b"ERR invalid UTF-8".to_vec());
            };
            let Ok(count) = count_text.parse::<usize>() else {
                return RespValue::Error(b"ERR invalid integer".to_vec());
            };
            Some(count)
        }
        _ => return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec()),
    };

    let mut store = store.lock().unwrap();
    match store.get_mut(&key) {
        Some(entry) => {
            if let Value::List(list) = &mut entry.value {
                let Some(count) = count else {
                    let return_value = match list.pop_front() {
                        Some(value) => RespValue::BulkString(value),
                        None => RespValue::NullBulkString,
                    };

                    if list.is_empty() {
                        store.remove(&key);
                    };

                    return return_value;
                };

                if list.is_empty() || count == 0 {
                    RespValue::Array(Vec::new())
                } else {
                    let count = count.min(list.len());

                    let mut response = Vec::new();
                    for _ in 0..count {
                        response.push(RespValue::BulkString(list.pop_front().unwrap()));
                    }

                    if list.is_empty() {
                        store.remove(&key);
                    };

                    RespValue::Array(response)
                }
            } else {
                RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec())
            }
        }
        None => RespValue::NullBulkString,
    }
}

fn llen(mut args: CommandArgs, store: &Store) -> RespValue {
    let (Some(BulkString(key)), None) = (args.next(), args.next()) else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let store = store.lock().unwrap();
    match store.get(&key) {
        Some(entry) => {
            if let Value::List(list) = &entry.value {
                RespValue::Integer(list.len() as i64)
            } else {
                RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec())
            }
        }
        None => RespValue::Integer(0),
    }
}

fn lpush(mut args: CommandArgs, store: &Store) -> RespValue {
    let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next()) else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let mut store = store.lock().unwrap();
    match store.get_mut(&key) {
        Some(entry) => {
            if let Value::List(list) = &mut entry.value {
                list.push_front(value);

                while let Some(BulkString(value)) = args.next() {
                    list.push_front(value);
                }

                RespValue::Integer(list.len() as i64)
            } else {
                RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec())
            }
        }
        None => {
            let mut list = VecDeque::new();
            list.push_front(value);

            while let Some(BulkString(value)) = args.next() {
                list.push_front(value);
            }

            let len = list.len();

            store.insert(
                key,
                Entry {
                    value: Value::List(list),
                    expires_at: None,
                },
            );

            RespValue::Integer(len as i64)
        }
    }
}

fn lrange(mut args: CommandArgs, store: &Store) -> RespValue {
    let (Some(BulkString(list_key)), Some(BulkString(start)), Some(BulkString(stop)), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let (Ok(start_str), Ok(stop_str)) = (std::str::from_utf8(&start), std::str::from_utf8(&stop))
    else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let (Ok(mut start), Ok(mut stop)) = (start_str.parse::<isize>(), stop_str.parse::<isize>())
    else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let store = store.lock().unwrap();
    match store.get(&list_key) {
        Some(entry) => {
            if let Value::List(list) = &entry.value {
                let len = match isize::try_from(list.len()) {
                    Ok(len) => len,
                    Err(e) => {
                        return RespValue::Error(
                            format!("ERR list len couldn't convert to isize: {}", e).into_bytes(),
                        );
                    }
                };

                if list.is_empty() {
                    RespValue::Array(Vec::new())
                } else {
                    if start < 0 {
                        start = (len + start).max(0);
                    }
                    stop = if stop < 0 { len + stop } else { stop };

                    if start >= len || stop < 0 || start > stop {
                        RespValue::Array(Vec::new())
                    } else {
                        stop = stop.min(len - 1);
                        let count = (stop - start + 1) as usize;

                        let items = list
                            .iter()
                            .skip(start as usize)
                            .take(count)
                            .cloned()
                            .map(RespValue::BulkString)
                            .collect();
                        RespValue::Array(items)
                    }
                }
            } else {
                RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec())
            }
        }
        None => RespValue::Array(Vec::new()),
    }
}

fn rpush(mut args: CommandArgs, store: &Store) -> RespValue {
    let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next()) else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let mut store = store.lock().unwrap();
    match store.get_mut(&key) {
        Some(entry) => {
            if let Value::List(list) = &mut entry.value {
                list.push_back(value);

                while let Some(BulkString(value)) = args.next() {
                    list.push_back(value);
                }

                RespValue::Integer(list.len() as i64)
            } else {
                RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec())
            }
        }
        None => {
            let mut list = VecDeque::new();
            list.push_back(value);

            while let Some(BulkString(value)) = args.next() {
                list.push_back(value);
            }

            let len = list.len();

            store.insert(
                key,
                Entry {
                    value: Value::List(list),
                    expires_at: None,
                },
            );

            RespValue::Integer(len as i64)
        }
    }
}

fn set(mut args: CommandArgs, store: &Store) -> RespValue {
    let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next()) else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let expiry_bytes = match args.next() {
        None => None,
        Some(BulkString(option)) if option.eq_ignore_ascii_case(b"PX") => {
            let (Some(BulkString(millis)), None) = (args.next(), args.next()) else {
                return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
            };
            Some(millis)
        }
        _ => return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec()),
    };

    let expires_at = match expiry_bytes {
        None => None,
        Some(bytes) => {
            let deadline = std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .filter(|&millis| millis > 0)
                .and_then(|millis| Instant::now().checked_add(Duration::from_millis(millis)));

            let Some(deadline) = deadline else {
                return RespValue::Error(b"ERR invalid expire time in 'set' command".to_vec());
            };

            Some(deadline)
        }
    };

    store.lock().unwrap().insert(
        key,
        Entry {
            value: Value::String(value),
            expires_at,
        },
    );
    RespValue::SimpleString(b"OK".to_vec())
}

fn get(mut args: CommandArgs, store: &Store) -> RespValue {
    let (Some(BulkString(key)), None) = (args.next(), args.next()) else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    let mut store = store.lock().unwrap();
    match store.get(&key) {
        Some(entry)
            if entry
                .expires_at
                .is_some_and(|expiry| Instant::now() >= expiry) =>
        {
            store.remove(&key);
            RespValue::NullBulkString
        }
        Some(entry) => {
            if let Value::String(value) = &entry.value {
                RespValue::BulkString(value.clone())
            } else {
                RespValue::Error(
                    b"WRONGTYPE Operation against a key holding the wrong kind of value".to_vec(),
                )
            }
        }
        None => RespValue::NullBulkString,
    }
}

fn echo(mut args: CommandArgs) -> RespValue {
    let (Some(BulkString(argument)), None) = (args.next(), args.next()) else {
        return RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec());
    };

    RespValue::BulkString(argument)
}

fn ping(mut args: CommandArgs) -> RespValue {
    if args.next().is_none() {
        RespValue::SimpleString(b"PONG".to_vec())
    } else {
        RespValue::Error(COMMAND_ERROR_MESSAGE.to_vec())
    }
}
