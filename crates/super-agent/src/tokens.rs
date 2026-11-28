//! The token estimate used where no provider count is at hand.

/// Bytes of text the estimate counts as one token.
pub const BYTES_PER_TOKEN: u64 = 4;

/// About one token per four bytes of text. Compaction, the context meter
/// and the `Read` limits all count with this.
pub fn estimate_tokens_text(text: &str) -> u64 {
    (text.len() as u64).div_ceil(BYTES_PER_TOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bytes_make_a_token() {
        assert_eq!(estimate_tokens_text(""), 0);
        assert_eq!(estimate_tokens_text("abc"), 1);
        assert_eq!(estimate_tokens_text("abcd"), 1);
        assert_eq!(estimate_tokens_text("abcde"), 2);
    }
}
