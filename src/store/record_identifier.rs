//! Record-identifier minting: the production-compatible short/base36 keys
//! Spirit owns because migration must preserve them as stable record keys.
//!
//! The mint holds the set of identifiers already in use; a code range owns
//! the value span and base36 rendering for a single code length. Both are
//! real data-bearing types — the mint cannot decide the next free identifier
//! without its `used_identifiers` set, and the range cannot render a code
//! without its `first_value`/`value_count`.

use std::collections::BTreeSet;

use crate::schema::sema::StoredRecord;

use super::StoreError;

pub(super) const RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH: usize = 4;
pub(super) const RECORD_IDENTIFIER_MAXIMUM_CODE_LENGTH: usize = 7;
pub(super) const RECORD_IDENTIFIER_CODE_RADIX: u64 = 36;
pub(super) const RANDOM_IDENTIFIER_ATTEMPTS_PER_LENGTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordIdentifierMint {
    used_identifiers: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordIdentifierCodeRange {
    first_value: u64,
    value_count: u64,
}

impl RecordIdentifierMint {
    pub(super) fn from_records(records: &[StoredRecord]) -> Self {
        Self {
            used_identifiers: records
                .iter()
                .map(|record| record.record_identifier.payload().clone())
                .collect(),
        }
    }

    pub(super) fn next_identifier(&self) -> Result<String, StoreError> {
        for code_length in
            RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH..=RECORD_IDENTIFIER_MAXIMUM_CODE_LENGTH
        {
            if let Some(identifier) = self.identifier_for_code_length(code_length)? {
                return Ok(identifier);
            }
        }
        Err(StoreError::IdentifierMint(format!(
            "no available record identifier code between {RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH} and {RECORD_IDENTIFIER_MAXIMUM_CODE_LENGTH} characters"
        )))
    }

    fn identifier_for_code_length(&self, code_length: usize) -> Result<Option<String>, StoreError> {
        let range = RecordIdentifierCodeRange::new(code_length);
        for _ in 0..RANDOM_IDENTIFIER_ATTEMPTS_PER_LENGTH {
            let identifier = range.random_identifier()?;
            if !self.used_identifiers.contains(&identifier) {
                return Ok(Some(identifier));
            }
        }
        Ok(range.first_available_identifier(&self.used_identifiers))
    }
}

impl RecordIdentifierCodeRange {
    fn new(code_length: usize) -> Self {
        let first_value = if code_length == RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH {
            0
        } else {
            Self::radix_power(code_length - 1)
        };
        let next_length_first_value = Self::radix_power(code_length);
        Self {
            first_value,
            value_count: next_length_first_value - first_value,
        }
    }

    fn random_identifier(self) -> Result<String, StoreError> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes)
            .map_err(|error| StoreError::IdentifierMint(error.to_string()))?;
        let offset = u64::from_be_bytes(bytes) % self.value_count;
        Ok(Self::code_from_value(self.first_value + offset))
    }

    fn first_available_identifier(self, used_identifiers: &BTreeSet<String>) -> Option<String> {
        let last_value = self.first_value + self.value_count;
        (self.first_value..last_value)
            .map(Self::code_from_value)
            .find(|identifier| !used_identifiers.contains(identifier))
    }

    fn code_from_value(mut value: u64) -> String {
        let mut digits = Vec::new();
        while value > 0 {
            let digit = (value % RECORD_IDENTIFIER_CODE_RADIX) as u8;
            digits.push(Self::digit_character(digit));
            value /= RECORD_IDENTIFIER_CODE_RADIX;
        }
        while digits.len() < RECORD_IDENTIFIER_MINIMUM_CODE_LENGTH {
            digits.push('0');
        }
        digits.iter().rev().collect()
    }

    fn digit_character(digit: u8) -> char {
        match digit {
            0..=9 => char::from(b'0' + digit),
            10..=35 => char::from(b'a' + digit - 10),
            _ => unreachable!("base36 digit is constrained by modulo"),
        }
    }

    fn radix_power(exponent: usize) -> u64 {
        (0..exponent).fold(1, |value, _| value * RECORD_IDENTIFIER_CODE_RADIX)
    }
}
