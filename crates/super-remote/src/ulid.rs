//! A ULID: 48 bits of time and 80 bits of randomness in Crockford base32.

use rand::RngCore as _;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn generate() -> String {
    let time = u128::from(crate::now_ms().max(0) as u64);
    let mut random = [0_u8; 10];
    rand::rng().fill_bytes(&mut random);
    let mut value: u128 = 0;
    for byte in random {
        value = (value << 8) | u128::from(byte);
    }
    value |= time << 80;
    // 26 characters cover 130 bits; the top two are always zero.
    let mut out = [b'0'; 26];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn ulids_are_26_sortable_characters() {
        let first = super::generate();
        let second = super::generate();
        assert_eq!(first.len(), 26);
        assert!(first.bytes().all(|b| super::ALPHABET.contains(&b)));
        assert_ne!(first, second);
        assert!(
            first[..10] <= second[..10],
            "time prefix must not go backwards"
        );
    }

    #[test]
    fn the_time_prefix_encodes_the_current_time() {
        let before = crate::now_ms() as u128;
        let ulid = super::generate();
        let after = crate::now_ms() as u128;
        let time = ulid[..10].bytes().fold(0_u128, |acc, byte| {
            let digit = super::ALPHABET
                .iter()
                .position(|&candidate| candidate == byte)
                .expect("alphabet");
            (acc << 5) | digit as u128
        });
        assert!(
            (before..=after).contains(&time),
            "{time} not in {before}..={after}"
        );
    }
}
