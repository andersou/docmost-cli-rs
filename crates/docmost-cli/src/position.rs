//! Fractional-indexing order keys, as used by Docmost for the `position` of
//! pages among their siblings. This is a port of `generateKeyBetween` from the
//! `fractional-indexing` JavaScript package (base62 digits, integer part
//! prefixed by a length letter, fractional part without trailing zeros).

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// Docmost validates `position` with `@MinLength(5) @MaxLength(12)`.
const MIN_POSITION_LEN: usize = 5;
const MAX_POSITION_LEN: usize = 12;

fn digit_index(digit: u8) -> Option<usize> {
    DIGITS.iter().position(|d| *d == digit)
}

fn integer_length(head: u8) -> Result<usize, String> {
    match head {
        b'a'..=b'z' => Ok(usize::from(head - b'a') + 2),
        b'A'..=b'Z' => Ok(usize::from(b'Z' - head) + 2),
        _ => Err(format!("invalid order key head: {}", head as char)),
    }
}

fn integer_part(key: &str) -> Result<&str, String> {
    let head = key.as_bytes().first().copied().ok_or("empty order key")?;
    let length = integer_length(head)?;
    if length > key.len() {
        return Err(format!("invalid order key: {key}"));
    }
    Ok(&key[..length])
}

fn validate(key: &str) -> Result<(), String> {
    let smallest = format!("A{}", "0".repeat(26));
    if key == smallest {
        return Err(format!("invalid order key: {key}"));
    }
    let integer = integer_part(key)?;
    let fraction = &key[integer.len()..];
    if fraction.ends_with('0') {
        return Err(format!("invalid order key: {key}"));
    }
    if !key.bytes().skip(1).all(|b| digit_index(b).is_some()) {
        return Err(format!("invalid order key: {key}"));
    }
    Ok(())
}

/// Midpoint between two fractional parts, where `b == None` means the
/// upper bound is the next integer.
fn midpoint(a: &str, b: Option<&str>) -> Result<String, String> {
    if let Some(b) = b
        && a >= b
    {
        return Err(format!("{a} >= {b}"));
    }
    if a.ends_with('0') || b.is_some_and(|b| b.ends_with('0')) {
        return Err("trailing zero".into());
    }
    if let Some(b) = b {
        let a_bytes = a.as_bytes();
        let b_bytes = b.as_bytes();
        let mut n = 0;
        while n < b_bytes.len() && a_bytes.get(n).copied().unwrap_or(b'0') == b_bytes[n] {
            n += 1;
        }
        if n > 0 {
            let rest = midpoint(&a[n.min(a.len())..], Some(&b[n..]))?;
            return Ok(format!("{}{rest}", &b[..n]));
        }
    }
    let digit_a = a
        .as_bytes()
        .first()
        .map(|d| digit_index(*d).ok_or("invalid digit"))
        .transpose()?
        .unwrap_or(0);
    let digit_b = b
        .and_then(|b| b.as_bytes().first())
        .map(|d| digit_index(*d).ok_or("invalid digit"))
        .transpose()?
        .unwrap_or(DIGITS.len());
    if digit_b - digit_a > 1 {
        let mid = (digit_a + digit_b).div_ceil(2);
        return Ok((DIGITS[mid] as char).to_string());
    }
    match b {
        Some(b) if b.len() > 1 => Ok(b[..1].to_owned()),
        _ => {
            let rest = midpoint(a.get(1..).unwrap_or(""), None)?;
            Ok(format!("{}{rest}", DIGITS[digit_a] as char))
        }
    }
}

fn increment_integer(x: &str) -> Result<Option<String>, String> {
    let head = x.as_bytes()[0];
    if x.len() != integer_length(head)? {
        return Err(format!("invalid integer part of order key: {x}"));
    }
    let mut digits: Vec<u8> = x.as_bytes()[1..].to_vec();
    let mut carry = true;
    for digit in digits.iter_mut().rev() {
        if !carry {
            break;
        }
        let d = digit_index(*digit).ok_or("invalid digit")? + 1;
        if d == DIGITS.len() {
            *digit = DIGITS[0];
        } else {
            *digit = DIGITS[d];
            carry = false;
        }
    }
    if !carry {
        return Ok(Some(format!(
            "{}{}",
            head as char,
            String::from_utf8_lossy(&digits)
        )));
    }
    match head {
        b'Z' => Ok(Some("a0".into())),
        b'z' => Ok(None),
        _ => {
            let next = head + 1;
            if next > b'a' {
                digits.push(DIGITS[0]);
            } else {
                digits.pop();
            }
            Ok(Some(format!(
                "{}{}",
                next as char,
                String::from_utf8_lossy(&digits)
            )))
        }
    }
}

fn decrement_integer(x: &str) -> Result<Option<String>, String> {
    let head = x.as_bytes()[0];
    if x.len() != integer_length(head)? {
        return Err(format!("invalid integer part of order key: {x}"));
    }
    let mut digits: Vec<u8> = x.as_bytes()[1..].to_vec();
    let mut borrow = true;
    for digit in digits.iter_mut().rev() {
        if !borrow {
            break;
        }
        match digit_index(*digit).ok_or("invalid digit")?.checked_sub(1) {
            None => *digit = DIGITS[DIGITS.len() - 1],
            Some(d) => {
                *digit = DIGITS[d];
                borrow = false;
            }
        }
    }
    if !borrow {
        return Ok(Some(format!(
            "{}{}",
            head as char,
            String::from_utf8_lossy(&digits)
        )));
    }
    match head {
        b'a' => Ok(Some("Zz".into())),
        b'A' => Ok(None),
        _ => {
            let previous = head - 1;
            if previous < b'Z' {
                digits.push(DIGITS[DIGITS.len() - 1]);
            } else {
                digits.pop();
            }
            Ok(Some(format!(
                "{}{}",
                previous as char,
                String::from_utf8_lossy(&digits)
            )))
        }
    }
}

/// Generates a key strictly between `a` (or the start when `None`) and `b`
/// (or the end when `None`).
pub fn generate_key_between(a: Option<&str>, b: Option<&str>) -> Result<String, String> {
    if let Some(a) = a {
        validate(a)?;
    }
    if let Some(b) = b {
        validate(b)?;
    }
    if let (Some(a), Some(b)) = (a, b)
        && a >= b
    {
        return Err(format!("{a} >= {b}"));
    }
    let Some(a) = a else {
        let Some(b) = b else {
            return Ok("a0".into());
        };
        let ib = integer_part(b)?;
        let fb = &b[ib.len()..];
        if ib == format!("A{}", "0".repeat(26)) {
            return Ok(format!("{ib}{}", midpoint("", Some(fb))?));
        }
        if ib < b {
            return Ok(ib.to_owned());
        }
        return decrement_integer(ib)?.ok_or_else(|| "cannot decrement any more".into());
    };
    let ia = integer_part(a)?;
    let fa = &a[ia.len()..];
    let Some(b) = b else {
        return match increment_integer(ia)? {
            Some(next) => Ok(next),
            None => Ok(format!("{ia}{}", midpoint(fa, None)?)),
        };
    };
    let ib = integer_part(b)?;
    let fb = &b[ib.len()..];
    if ia == ib {
        return Ok(format!("{ia}{}", midpoint(fa, Some(fb))?));
    }
    let next = increment_integer(ia)?.ok_or("cannot increment any more")?;
    if next.as_str() < b {
        Ok(next)
    } else {
        Ok(format!("{ia}{}", midpoint(fa, None)?))
    }
}

/// Normalises a sibling position read from the server so it can be used
/// as a bound: trailing zeros in the fractional part do not change the
/// ordering but are rejected by the reference algorithm.
pub fn normalize_position(position: &str) -> Result<String, String> {
    let integer = integer_part(position)?;
    let fraction = position[integer.len()..].trim_end_matches('0');
    let key = format!("{integer}{fraction}");
    validate(&key)?;
    Ok(key)
}

/// A key between `a` and `b` that also satisfies the 5..12 character
/// window Docmost enforces on `pages/move`.
pub fn position_between(a: Option<&str>, b: Option<&str>) -> Result<String, String> {
    let mut key = generate_key_between(a, b)?;
    // Padding may only be appended when the key is not a prefix of the
    // upper bound; otherwise the padded key could sort after it.
    if let Some(b) = b {
        let mut guard = 0;
        while b.starts_with(&key) {
            key = generate_key_between(Some(&key), Some(b))?;
            guard += 1;
            if guard > 8 {
                return Err(format!("unable to find a position between {a:?} and {b}"));
            }
        }
    }
    while key.len() < MIN_POSITION_LEN {
        key.push('V');
    }
    if key.len() > MAX_POSITION_LEN {
        return Err(format!(
            "computed position {key} is longer than {MAX_POSITION_LEN} characters; pass --position explicitly"
        ));
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(a: Option<&str>, b: Option<&str>, expected: &str) {
        match generate_key_between(a, b) {
            Ok(key) => assert_eq!(key, expected, "between {a:?} and {b:?}"),
            Err(error) => assert_eq!(error, expected, "between {a:?} and {b:?}"),
        }
    }

    #[test]
    fn matches_reference_vectors() {
        check(None, None, "a0");
        check(None, Some("a0"), "Zz");
        check(None, Some("Zz"), "Zy");
        check(Some("a0"), None, "a1");
        check(Some("a1"), None, "a2");
        check(Some("a0"), Some("a1"), "a0V");
        check(Some("a1"), Some("a2"), "a1V");
        check(Some("a0V"), Some("a1"), "a0l");
        check(Some("Zz"), Some("a0"), "ZzV");
        check(Some("Zz"), Some("a1"), "a0");
        check(None, Some("Y00"), "Xzzz");
        check(Some("bzz"), None, "c000");
        check(Some("a0"), Some("a0V"), "a0G");
        check(Some("a0"), Some("a0G"), "a08");
        check(Some("b125"), Some("b129"), "b127");
        check(Some("a0"), Some("a1V"), "a1");
        check(Some("Zz"), Some("a1V"), "a0");
        check(
            None,
            Some("A00000000000000000000000000"),
            "invalid order key: A00000000000000000000000000",
        );
        check(
            None,
            Some("A000000000000000000000000001"),
            "A000000000000000000000000000V",
        );
        check(
            Some("zzzzzzzzzzzzzzzzzzzzzzzzzzy"),
            None,
            "zzzzzzzzzzzzzzzzzzzzzzzzzzz",
        );
        check(
            Some("zzzzzzzzzzzzzzzzzzzzzzzzzzz"),
            None,
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzV",
        );
        check(Some("a00"), None, "invalid order key: a00");
        check(Some("a00"), Some("a1"), "invalid order key: a00");
        check(Some("0"), Some("1"), "invalid order key head: 0");
        check(Some("a1"), Some("a0"), "a1 >= a0");
    }

    #[test]
    fn positions_respect_docmost_length_window() {
        assert_eq!(position_between(None, None).unwrap(), "a0VVV");
        assert_eq!(position_between(Some("a0"), None).unwrap(), "a1VVV");
        assert_eq!(position_between(Some("a01TB"), None).unwrap(), "a1VVV");
        let between = position_between(Some("a0"), Some("a1")).unwrap();
        assert_eq!(between, "a0VVV");
        assert!("a0" < between.as_str() && between.as_str() < "a1");
        // `a0` is a prefix of `a0V`, so the key is regenerated before padding.
        let before = position_between(None, Some("a0V")).unwrap();
        assert!(before.as_str() < "a0V", "{before}");
        assert!(before.len() >= 5);
        let first = position_between(None, Some("a01TB")).unwrap();
        assert!(first.as_str() < "a01TB", "{first}");
        assert!(position_between(Some("zzzzzzzzzzzzzzzzzzzzzzzzzzz"), None).is_err());
    }

    #[test]
    fn normalizes_server_positions() {
        assert_eq!(normalize_position("a10").unwrap(), "a1");
        assert_eq!(normalize_position("a01TB").unwrap(), "a01TB");
        assert!(normalize_position("00").is_err());
    }
}
