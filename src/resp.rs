use thiserror::Error;

#[derive(Debug, Error)]
enum RespError {
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

fn parse_resp(buf: &[u8], pos: usize) -> Result<Option<(Vec<u8>, usize)>, RespError> {
    let byte = match buf.get(pos) {
        Some(byte) => *byte,
        None => return Ok(None),
    };

    match byte {
        b'$' => parse_bulk_string(buf, pos + 1),
        b => Err(RespError::UnsupportedType(b)),
    }
}

fn parse_bulk_string(buf: &[u8], pos: usize) -> Result<Option<(Vec<u8>, usize)>, RespError> {
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

    Ok(Some((buf[payload_start..payload_end].to_vec(), next_pos)))
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

    let digit_count = buf.iter().take_while(|b| b.is_ascii_digit()).count();

    if buf.is_empty() {
        return Ok(None);
    }

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

        assert_eq!(result, Some((b"hey".to_vec(), 9)));
    }

    #[test]
    fn parse_normal_bulk_string_2() {
        let input = b"$5\r\nhello\r\n";

        assert_eq!(parse_resp(input, 0).unwrap(), Some((b"hello".to_vec(), 11)));
    }

    #[test]
    fn parse_incomplete_bulk_string() {
        let incomplete = b"$5\r\nhel";

        assert_eq!(parse_resp(incomplete, 0).unwrap(), None);
    }
}
