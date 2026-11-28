use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RespValue {
    BulkString(Vec<u8>),   // $5\r\nhello\r\n
    Array(Vec<RespValue>), // *2\r\n$1\r\na\r\n$1\r\nb\r\n
    SimpleString(Vec<u8>), // +OK\r\n
    Integer(i64),          // :42\r\n
    Error(Vec<u8>),        // -ERR invalid integer\r\n
    NullBulkString,        // $-1\r\n
    NullArray,             // *-1\r\n
}

impl RespValue {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        self.encode_into(&mut buffer);
        buffer
    }

    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::BulkString(bytes) => {
                let header = format!("${}\r\n", bytes.len());
                buffer.reserve(header.len() + bytes.len() + 2);
                buffer.extend_from_slice(header.as_bytes());
                buffer.extend_from_slice(bytes);
                buffer.extend_from_slice(b"\r\n");
            }
            Self::Array(vec) => {
                let header = format!("*{}\r\n", vec.len());
                buffer.extend_from_slice(header.as_bytes());

                for item in vec {
                    item.encode_into(buffer);
                }
            }
            Self::SimpleString(bytes) => {
                buffer.extend_from_slice(b"+");
                buffer.extend_from_slice(bytes);
                buffer.extend_from_slice(b"\r\n");
            }
            Self::Integer(int) => {
                buffer.extend_from_slice(b":");
                buffer.extend_from_slice(int.to_string().as_bytes());
                buffer.extend_from_slice(b"\r\n");
            }
            Self::Error(bytes) => {
                buffer.extend_from_slice(b"-");
                buffer.extend_from_slice(bytes);
                buffer.extend_from_slice(b"\r\n");
            }
            Self::NullBulkString => {
                buffer.extend_from_slice(b"$-1\r\n");
            }
            Self::NullArray => {
                buffer.extend_from_slice(b"*-1\r\n");
            }
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum RespError {
    #[error("invalid UTF-8")]
    Utf8Error(#[from] std::str::Utf8Error),

    #[error("invalid integer: {0}")]
    InvalidInteger(#[from] std::num::ParseIntError),

    #[error("expected CRLF")]
    ExpectedCrlf,

    #[error("expected length")]
    ExpectedLength,

    #[error("unsupported type: {0}")]
    UnsupportedType(u8),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum HandlerError {
    #[error(transparent)]
    Parse(#[from] RespError),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub(crate) struct RespHandler {
    stream: TcpStream,
    pending: Vec<u8>,
}

impl RespHandler {
    pub(crate) fn new(stream: TcpStream) -> Self {
        RespHandler {
            stream,
            pending: Vec::new(),
        }
    }

    pub(crate) async fn read_value(&mut self) -> Result<Option<RespValue>, HandlerError> {
        loop {
            if let Some((value, next_pos)) = parse_resp(&self.pending, 0)? {
                self.pending.drain(..next_pos);
                return Ok(Some(value));
            }

            let mut chunk = [0u8; 512];

            let bytes_read = self.stream.read(&mut chunk).await?;
            if bytes_read == 0 {
                return Ok(None);
            }

            self.pending.extend_from_slice(&chunk[..bytes_read]);
        }
    }

    pub(crate) async fn write_reply(&mut self, reply: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(reply).await
    }
}

pub(crate) type ParseResult = Result<Option<(RespValue, usize)>, RespError>;

pub(crate) fn parse_resp(buf: &[u8], pos: usize) -> ParseResult {
    let byte = match buf.get(pos) {
        Some(byte) => *byte,
        None => return Ok(None),
    };

    match byte {
        b'$' => parse_bulk_string(buf, pos + 1),
        b'*' => parse_array(buf, pos + 1),
        b => Err(RespError::UnsupportedType(b)),
    }
}

// when a later element is incomplete all previously parsed
// elements are dropped.
// todo: preserve previously parsed elements
fn parse_array(buf: &[u8], pos: usize) -> ParseResult {
    let Some((payload_length, pos)) = parse_length(buf, pos)? else {
        return Ok(None);
    };

    let Some(mut next_pos) = parse_crlf(buf, pos)? else {
        return Ok(None);
    };

    let mut array = Vec::new();

    for _ in 0..payload_length {
        let Some((value, pos)) = parse_resp(buf, next_pos)? else {
            return Ok(None);
        };

        next_pos = pos;
        array.push(value);
    }

    Ok(Some((RespValue::Array(array), next_pos)))
}

fn parse_bulk_string(buf: &[u8], pos: usize) -> ParseResult {
    let Some((payload_length, pos)) = parse_length(buf, pos)? else {
        return Ok(None);
    };

    let Some(payload_start) = parse_crlf(buf, pos)? else {
        return Ok(None);
    };

    let payload_end = payload_start + payload_length as usize;

    let Some(next_pos) = parse_crlf(buf, payload_end)? else {
        return Ok(None);
    };

    Ok(Some((
        RespValue::BulkString(buf[payload_start..payload_end].to_vec()),
        next_pos,
    )))
}

fn parse_crlf(buf: &[u8], pos: usize) -> Result<Option<usize>, RespError> {
    if buf.len() < pos + 2 {
        return Ok(None);
    }

    if &buf[pos..pos + 2] != b"\r\n" {
        return Err(RespError::ExpectedCrlf);
    }

    Ok(Some(pos + 2))
}

fn parse_length(buf: &[u8], pos: usize) -> Result<Option<(i64, usize)>, RespError> {
    let buf = match buf.get(pos..) {
        Some(buf) => buf,
        None => return Ok(None),
    };

    if buf.is_empty() {
        return Ok(None);
    }

    let digit_count = buf.iter().take_while(|b| b.is_ascii_digit()).count();

    if digit_count == 0 {
        return Err(RespError::ExpectedLength);
    }

    let digit_bytes = &buf[..digit_count];

    let digits = std::str::from_utf8(digit_bytes)?;
    let length = digits.parse::<i64>()?;

    Ok(Some((length, pos + digit_count)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_bulk_string() {
        let input = b"$3\r\nhey\r\n";

        let result = parse_resp(input, 0).unwrap();

        assert_eq!(result, Some((RespValue::BulkString(b"hey".to_vec()), 9)));
    }

    #[test]
    fn parse_normal_bulk_string_2() {
        let input = b"$5\r\nhello\r\n";

        assert_eq!(
            parse_resp(input, 0).unwrap(),
            Some((RespValue::BulkString(b"hello".to_vec()), 11))
        );
    }

    #[test]
    fn parse_incomplete_bulk_string() {
        let incomplete = b"$5\r\nhel";

        assert_eq!(parse_resp(incomplete, 0).unwrap(), None);
    }

    #[test]
    fn parse_empty_bulk_string() {
        assert_eq!(
            parse_resp(b"$0\r\n\r\n", 0).unwrap(),
            Some((RespValue::BulkString(Vec::new()), 6))
        );
    }

    #[test]
    fn parse_bulk_string_with_multidigit_length() {
        assert_eq!(
            parse_resp(b"$12\r\nhello world!\r\n", 0).unwrap(),
            Some((RespValue::BulkString(b"hello world!".to_vec()), 19))
        );
    }

    #[test]
    fn parse_bulk_string_with_binary_payload_and_embedded_crlf() {
        assert_eq!(
            parse_resp(b"$4\r\n\xff\r\n\0\r\n", 0).unwrap(),
            Some((RespValue::BulkString(b"\xff\r\n\0".to_vec()), 10))
        );
    }

    #[test]
    fn parse_echo_array() {
        let input = b"*2\r\n$4\r\nECHO\r\n$3\r\nhey\r\n";
        let expected = RespValue::Array(vec![
            RespValue::BulkString(b"ECHO".to_vec()),
            RespValue::BulkString(b"hey".to_vec()),
        ]);

        assert_eq!(parse_resp(input, 0).unwrap(), Some((expected, 23)));
    }

    #[test]
    fn parse_empty_array() {
        assert_eq!(
            parse_resp(b"*0\r\n", 0).unwrap(),
            Some((RespValue::Array(Vec::new()), 4))
        );
    }

    #[test]
    fn parse_nested_array() {
        let input = b"*3\r\n*1\r\n$3\r\nhey\r\n*0\r\n$0\r\n\r\n";
        let expected = RespValue::Array(vec![
            RespValue::Array(vec![RespValue::BulkString(b"hey".to_vec())]),
            RespValue::Array(Vec::new()),
            RespValue::BulkString(Vec::new()),
        ]);

        assert_eq!(parse_resp(input, 0).unwrap(), Some((expected, 27)));
    }

    #[test]
    fn parse_array_from_nonzero_position() {
        let input = b"prefix*1\r\n$3\r\nhey\r\n";
        let expected = RespValue::Array(vec![RespValue::BulkString(b"hey".to_vec())]);

        assert_eq!(parse_resp(input, 6).unwrap(), Some((expected, 19)));
    }

    #[test]
    fn parse_array_leaves_following_value_unconsumed() {
        let input = b"*1\r\n$3\r\nhey\r\n$2\r\nhi\r\n";
        let (value, next_pos) = parse_resp(input, 0).unwrap().unwrap();

        assert_eq!(
            value,
            RespValue::Array(vec![RespValue::BulkString(b"hey".to_vec())])
        );
        assert_eq!(next_pos, 13);
        assert_eq!(
            parse_resp(input, next_pos).unwrap(),
            Some((RespValue::BulkString(b"hi".to_vec()), 21))
        );
    }

    #[test]
    fn parse_incomplete_array_prefixes() {
        let input = b"*2\r\n$4\r\nECHO\r\n$3\r\nhey\r\n";

        // Covers partial headers, a missing child, and a partial second child.
        for end in 0..input.len() {
            assert_eq!(
                parse_resp(&input[..end], 0),
                Ok(None),
                "expected incomplete input for prefix ending at {end}"
            );
        }
    }

    #[test]
    fn parse_array_rejects_malformed_input() {
        let cases: &[(&[u8], RespError)] = &[
            (b"*x\r\n", RespError::ExpectedLength),
            (b"*1xx$0\r\n\r\n", RespError::ExpectedCrlf),
            (b"*1\r\n?", RespError::UnsupportedType(b'?')),
            (b"*1\r\n$3\r\nheyXX", RespError::ExpectedCrlf),
            (b"*1\r\n*1\r\n?", RespError::UnsupportedType(b'?')),
        ];

        for (input, expected_error) in cases {
            assert_eq!(
                parse_resp(input, 0).as_ref().unwrap_err(),
                expected_error,
                "expected an error for {input:?}"
            );
        }
    }
}
