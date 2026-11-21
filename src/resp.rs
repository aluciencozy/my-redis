use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RespValue {
    BulkString(Vec<u8>),
    Array(Vec<RespValue>),
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
