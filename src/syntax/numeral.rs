//! Which numerals Lua accepts, following `luaO_str2num`.
//!
//! A numeral is a decimal or hexadecimal integer, or a decimal or hexadecimal float. The
//! value is not computed: the printer writes numerals back as they were written, so the
//! compiled constant is the one Lua itself would produce.

/// Whether Lua's lexer accepts `text` as a numeral.
pub fn is_valid(text: &str) -> bool {
    let bytes = text.as_bytes();
    let hex = bytes.len() > 1 && bytes[0] == b'0' && matches!(bytes[1], b'x' | b'X');
    if hex {
        mantissa_and_exponent(&bytes[2..], |b| b.is_ascii_hexdigit(), b"pP")
    } else {
        mantissa_and_exponent(bytes, |b| b.is_ascii_digit(), b"eE")
    }
}

/// `digits [. digits] [exp [+-] decimal digits]`, with at least one mantissa digit.
fn mantissa_and_exponent(bytes: &[u8], digit: impl Fn(u8) -> bool, exponent: &[u8]) -> bool {
    let mut i = 0;
    let mut digits = 0;
    while i < bytes.len() && digit(bytes[i]) {
        i += 1;
        digits += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && digit(bytes[i]) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if i == bytes.len() {
        return true;
    }
    if !exponent.contains(&bytes[i]) {
        return false;
    }
    i += 1;
    if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
        i += 1;
    }
    let rest = &bytes[i..];
    !rest.is_empty() && rest.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::is_valid;

    #[test]
    fn accepts_the_numerals_lua_accepts() {
        for text in [
            "0",
            "42",
            "3.",
            ".5",
            "3.25",
            "1e10",
            "1E-3",
            "2e+4",
            "0x10",
            "0XfF",
            "0x.8",
            "0x1p4",
            "0x1.8P-1",
            "9223372036854775808",
            "0xffffffffffffffffff",
        ] {
            assert!(is_valid(text), "{text}");
        }
    }

    #[test]
    fn rejects_malformed_numerals() {
        for text in [
            "0x", "1e", "1e+", "3..2", "1.2.3", "0x1g", "12a", "0xp1", ".",
        ] {
            assert!(!is_valid(text), "{text}");
        }
    }
}
