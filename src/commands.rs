use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::resp::{
    RespValue,
    RespValue::{Array, BulkString},
};

use super::{Entry, Store, Value};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum CommandError {
    #[error("wrong number of arguments")]
    WrongNumberOfArguments,

    #[error("unsupported command")]
    UnsupportedCommand,

    #[error("wrong type")]
    WrongType,

    #[error("invalid UTF-8")]
    InvalidUtf8,

    #[error("invalid integer")]
    InvalidInteger,

    #[error("invalid expire time in 'set' command")]
    InvalidExpireTime,

    #[error("list length out of range")]
    ListLengthOutOfRange,
}

impl CommandError {
    pub(super) fn into_reply(self) -> RespValue {
        let message: &[u8] = match self {
            Self::WrongNumberOfArguments => b"ERR wrong number of arguments",
            Self::UnsupportedCommand => b"ERR unsupported command",
            Self::WrongType => b"WRONGTYPE Operation against a key holding the wrong kind of value",
            Self::InvalidUtf8 => b"ERR invalid UTF-8",
            Self::InvalidInteger => b"ERR invalid integer",
            Self::InvalidExpireTime => b"ERR invalid expire time in 'set' command",
            Self::ListLengthOutOfRange => b"ERR list length out of range",
        };
        RespValue::Error(message.to_vec())
    }
}

type CommandArgs = std::vec::IntoIter<RespValue>;

pub(super) fn command_reply(
    request: RespValue,
    store: &Store,
) -> Result<Option<RespValue>, CommandError> {
    let Array(items) = request else {
        return Ok(None);
    };

    let mut args = items.into_iter();
    let Some(BulkString(command)) = args.next() else {
        return Err(CommandError::UnsupportedCommand);
    };

    let reply = match command.as_slice() {
        command if command.eq_ignore_ascii_case(b"LPOP") => lpop(args, store)?,
        command if command.eq_ignore_ascii_case(b"LLEN") => llen(args, store)?,
        command if command.eq_ignore_ascii_case(b"LPUSH") => lpush(args, store)?,
        command if command.eq_ignore_ascii_case(b"LRANGE") => lrange(args, store)?,
        command if command.eq_ignore_ascii_case(b"RPUSH") => rpush(args, store)?,
        command if command.eq_ignore_ascii_case(b"SET") => set(args, store)?,
        command if command.eq_ignore_ascii_case(b"GET") => get(args, store)?,
        command if command.eq_ignore_ascii_case(b"ECHO") => echo(args)?,
        command if command.eq_ignore_ascii_case(b"PING") => ping(args)?,
        _ => return Err(CommandError::UnsupportedCommand),
    };

    Ok(Some(reply))
}

fn lpop(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let Some(BulkString(key)) = args.next() else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let count = match (args.next(), args.next()) {
        (None, None) => None,
        (Some(BulkString(count)), None) => {
            let Ok(count_text) = std::str::from_utf8(&count) else {
                return Err(CommandError::InvalidUtf8);
            };
            let Ok(count) = count_text.parse::<usize>() else {
                return Err(CommandError::InvalidInteger);
            };
            Some(count)
        }
        _ => return Err(CommandError::WrongNumberOfArguments),
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

                    return Ok(return_value);
                };

                if list.is_empty() || count == 0 {
                    Ok(RespValue::Array(Vec::new()))
                } else {
                    let count = count.min(list.len());

                    let mut response = Vec::new();
                    for _ in 0..count {
                        response.push(RespValue::BulkString(list.pop_front().unwrap()));
                    }

                    if list.is_empty() {
                        store.remove(&key);
                    };

                    Ok(RespValue::Array(response))
                }
            } else {
                Err(CommandError::WrongType)
            }
        }
        None => Ok(RespValue::NullBulkString),
    }
}

fn llen(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let (Some(BulkString(key)), None) = (args.next(), args.next()) else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let store = store.lock().unwrap();
    match store.get(&key) {
        Some(entry) => match &entry.value {
            Value::List(list) => Ok(RespValue::Integer(list.len() as i64)),
            _ => Err(CommandError::WrongType),
        },
        None => Ok(RespValue::Integer(0)),
    }
}

fn lpush(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next()) else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let mut store = store.lock().unwrap();
    match store.get_mut(&key) {
        Some(entry) => {
            if let Value::List(list) = &mut entry.value {
                list.push_front(value);

                while let Some(BulkString(value)) = args.next() {
                    list.push_front(value);
                }

                Ok(RespValue::Integer(list.len() as i64))
            } else {
                Err(CommandError::WrongType)
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

            Ok(RespValue::Integer(len as i64))
        }
    }
}

fn lrange(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let (Some(BulkString(list_key)), Some(BulkString(start)), Some(BulkString(stop)), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let (Ok(start_str), Ok(stop_str)) = (std::str::from_utf8(&start), std::str::from_utf8(&stop))
    else {
        return Err(CommandError::InvalidUtf8);
    };

    let (Ok(mut start), Ok(mut stop)) = (start_str.parse::<isize>(), stop_str.parse::<isize>())
    else {
        return Err(CommandError::InvalidInteger);
    };

    let store = store.lock().unwrap();
    match store.get(&list_key) {
        Some(entry) => {
            if let Value::List(list) = &entry.value {
                let len =
                    isize::try_from(list.len()).map_err(|_| CommandError::ListLengthOutOfRange)?;

                if list.is_empty() {
                    Ok(RespValue::Array(Vec::new()))
                } else {
                    if start < 0 {
                        start = (len + start).max(0);
                    }
                    stop = if stop < 0 { len + stop } else { stop };

                    if start >= len || stop < 0 || start > stop {
                        Ok(RespValue::Array(Vec::new()))
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
                        Ok(RespValue::Array(items))
                    }
                }
            } else {
                Err(CommandError::WrongType)
            }
        }
        None => Ok(RespValue::Array(Vec::new())),
    }
}

fn rpush(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next()) else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let mut store = store.lock().unwrap();
    match store.get_mut(&key) {
        Some(entry) => {
            if let Value::List(list) = &mut entry.value {
                list.push_back(value);

                while let Some(BulkString(value)) = args.next() {
                    list.push_back(value);
                }

                Ok(RespValue::Integer(list.len() as i64))
            } else {
                Err(CommandError::WrongType)
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

            Ok(RespValue::Integer(len as i64))
        }
    }
}

fn set(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let (Some(BulkString(key)), Some(BulkString(value))) = (args.next(), args.next()) else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let expiry_bytes = match args.next() {
        None => None,
        Some(BulkString(option)) if option.eq_ignore_ascii_case(b"PX") => {
            let (Some(BulkString(millis)), None) = (args.next(), args.next()) else {
                return Err(CommandError::WrongNumberOfArguments);
            };
            Some(millis)
        }
        _ => return Err(CommandError::WrongNumberOfArguments),
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
                return Err(CommandError::InvalidExpireTime);
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

    Ok(RespValue::SimpleString(b"OK".to_vec()))
}

fn get(mut args: CommandArgs, store: &Store) -> Result<RespValue, CommandError> {
    let (Some(BulkString(key)), None) = (args.next(), args.next()) else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    let mut store = store.lock().unwrap();
    match store.get(&key) {
        Some(entry)
            if entry
                .expires_at
                .is_some_and(|expiry| Instant::now() >= expiry) =>
        {
            store.remove(&key);
            Ok(RespValue::NullBulkString)
        }
        Some(entry) => {
            if let Value::String(value) = &entry.value {
                Ok(RespValue::BulkString(value.clone()))
            } else {
                Err(CommandError::WrongType)
            }
        }
        None => Ok(RespValue::NullBulkString),
    }
}

fn echo(mut args: CommandArgs) -> Result<RespValue, CommandError> {
    let (Some(BulkString(argument)), None) = (args.next(), args.next()) else {
        return Err(CommandError::WrongNumberOfArguments);
    };

    Ok(RespValue::BulkString(argument))
}

fn ping(mut args: CommandArgs) -> Result<RespValue, CommandError> {
    if args.next().is_none() {
        Ok(RespValue::SimpleString(b"PONG".to_vec()))
    } else {
        Err(CommandError::WrongNumberOfArguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(bytes: &[u8]) -> RespValue {
        BulkString(bytes.to_vec())
    }

    fn run(store: &Store, parts: &[&[u8]]) -> Result<RespValue, CommandError> {
        let request = Array(parts.iter().map(|part| bulk(part)).collect());
        command_reply(request, store).map(|reply| reply.expect("expected a command reply"))
    }

    #[test]
    fn ping_accepts_mixed_case_and_returns_pong() {
        let store = Store::default();

        assert_eq!(
            run(&store, &[b"PiNg"]).unwrap(),
            RespValue::SimpleString(b"PONG".to_vec())
        );
    }

    #[test]
    fn echo_preserves_binary_bytes() {
        let store = Store::default();

        assert_eq!(
            run(&store, &[b"ECHO", b"\xff\r\n"]).unwrap(),
            bulk(b"\xff\r\n")
        );
    }

    #[test]
    fn set_stores_bytes_that_get_returns() {
        let store = Store::default();

        assert_eq!(
            run(&store, &[b"SET", b"key", b"\xff\0"]).unwrap(),
            RespValue::SimpleString(b"OK".to_vec())
        );
        assert_eq!(run(&store, &[b"GET", b"key"]).unwrap(), bulk(b"\xff\0"));
    }

    #[test]
    fn get_returns_null_for_missing_and_expired_keys() {
        let store = Store::default();
        store.lock().unwrap().insert(
            b"expired".to_vec(),
            Entry {
                value: Value::String(b"old".to_vec()),
                expires_at: Some(Instant::now()),
            },
        );

        for key in [b"missing", b"expired"] {
            assert_eq!(
                run(&store, &[b"GET", key]).unwrap(),
                RespValue::NullBulkString
            );
        }
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn set_with_zero_expiry_preserves_existing_value() {
        let store = Store::default();
        run(&store, &[b"SET", b"key", b"old"]).unwrap();

        assert_eq!(
            run(&store, &[b"SET", b"key", b"new", b"PX", b"0"]),
            Err(CommandError::InvalidExpireTime)
        );
        assert_eq!(run(&store, &[b"GET", b"key"]).unwrap(), bulk(b"old"));
    }

    #[test]
    fn lpush_prepends_and_rpush_appends_items_in_order() {
        let store = Store::default();

        assert_eq!(
            run(&store, &[b"RPUSH", b"list", b"c", b"d"]).unwrap(),
            RespValue::Integer(2)
        );
        assert_eq!(
            run(&store, &[b"LPUSH", b"list", b"b", b"a"]).unwrap(),
            RespValue::Integer(4)
        );
        assert_eq!(
            run(&store, &[b"LLEN", b"list"]).unwrap(),
            RespValue::Integer(4)
        );
        assert_eq!(
            run(&store, &[b"LRANGE", b"list", b"0", b"-1"]).unwrap(),
            Array(vec![bulk(b"a"), bulk(b"b"), bulk(b"c"), bulk(b"d")])
        );
    }

    #[test]
    fn lrange_uses_inclusive_negative_indices() {
        let store = Store::default();
        run(&store, &[b"RPUSH", b"list", b"a", b"b", b"c", b"d"]).unwrap();

        assert_eq!(
            run(&store, &[b"LRANGE", b"list", b"-3", b"-2"]).unwrap(),
            Array(vec![bulk(b"b"), bulk(b"c")])
        );
    }

    #[test]
    fn lrange_rejects_extra_arguments() {
        let store = Store::default();
        run(&store, &[b"RPUSH", b"list", b"a", b"b", b"c", b"d"]).unwrap();

        assert_eq!(
            run(&store, &[b"LRANGE", b"list", b"-3", b"-2", b"0"]),
            Err(CommandError::WrongNumberOfArguments),
        );
    }

    #[test]
    fn lpop_with_count_returns_an_array_then_single_pop_removes_last_item() {
        let store = Store::default();
        run(&store, &[b"RPUSH", b"list", b"a", b"b", b"c"]).unwrap();

        assert_eq!(
            run(&store, &[b"LPOP", b"list", b"2"]).unwrap(),
            Array(vec![bulk(b"a"), bulk(b"b")])
        );
        assert_eq!(run(&store, &[b"LPOP", b"list"]).unwrap(), bulk(b"c"));
        assert_eq!(
            run(&store, &[b"LPOP", b"list"]).unwrap(),
            RespValue::NullBulkString
        );
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn lpop_rejects_a_noninteger_count_without_removing_items() {
        let store = Store::default();
        run(&store, &[b"RPUSH", b"list", b"a"]).unwrap();

        assert_eq!(
            run(&store, &[b"LPOP", b"list", b"nope"]),
            Err(CommandError::InvalidInteger)
        );
        assert_eq!(run(&store, &[b"LPOP", b"list"]).unwrap(), bulk(b"a"));
    }

    #[test]
    fn lpush_returns_wrong_type_for_a_string_key() {
        let store = Store::default();
        run(&store, &[b"SET", b"key", b"value"]).unwrap();

        assert_eq!(
            run(&store, &[b"LPUSH", b"key", b"item"]),
            Err(CommandError::WrongType)
        );
    }

    #[test]
    fn dispatcher_distinguishes_unknown_commands_from_extra_arguments() {
        let store = Store::default();

        assert_eq!(
            run(&store, &[b"UNKNOWN"]),
            Err(CommandError::UnsupportedCommand)
        );
        assert_eq!(
            run(&store, &[b"PING", b"extra"]),
            Err(CommandError::WrongNumberOfArguments)
        );
    }
}
