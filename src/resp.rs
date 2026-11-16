fn parse_bulk_string(buffer: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut pos = 0;

    if buffer[pos] != b'$' {
        return None;
    }

    pos += 1;
    let str_len = buffer[pos] - b'0';
    println!("{str_len}");
    pos += 1;

    if buffer[pos] != b'\r' || buffer[pos + 1] != b'\n' {
        return None;
    }

    pos += 2;
    let vec = buffer[pos..pos + str_len as usize].to_vec();
    pos += str_len as usize;

    if buffer[pos] != b'\r' || buffer[pos + 1] != b'\n' {
        return None;
    }

    Some((vec, pos + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_bulk_string() {
        let input = b"$3\r\nhey\r\n";

        let result = parse_bulk_string(input);

        assert_eq!(result, Some((b"hey".to_vec(), 9)));
    }

    #[test]
    fn parse_normal_bulk_string_2() {
        let input = b"$5\r\nhello\r\n";

        assert_eq!(parse_bulk_string(input), Some((b"hello".to_vec(), 11)));
    }

    #[test]
    fn parse_incomplete_bulk_string() {
        let incomplete = b"$5\r\nhel";

        assert_eq!(parse_bulk_string(incomplete), None);
    }
}
