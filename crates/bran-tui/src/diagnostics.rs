use std::{
    collections::{hash_map::DefaultHasher, VecDeque},
    hash::{Hash, Hasher},
};

const MAX_MESSAGE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticRecord {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub fingerprint: String,
    pub count: usize,
}

#[derive(Debug)]
pub struct DiagnosticStore {
    enabled: bool,
    capacity: usize,
    records: VecDeque<DiagnosticRecord>,
}

impl DiagnosticStore {
    pub fn new(enabled: bool, capacity: usize) -> Self {
        Self {
            enabled,
            capacity: capacity.max(1),
            records: VecDeque::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn records(&self) -> &VecDeque<DiagnosticRecord> {
        &self.records
    }

    pub fn record(&mut self, code: &str, severity: Severity, raw: &str) {
        if !self.enabled {
            return;
        }
        let message = bound(redact(raw));
        let fingerprint = fingerprint(&message);
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.code == code && record.fingerprint == fingerprint)
        {
            record.count += 1;
            return;
        }
        if self.records.len() == self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(DiagnosticRecord {
            code: code.to_owned(),
            severity,
            message,
            fingerprint,
            count: 1,
        });
    }

    pub fn export_text(&self) -> String {
        let occurrences: usize = self.records.iter().map(|record| record.count).sum();
        let mut output = format!(
            "diagnostics enabled={} capacity={} unique_count={} occurrence_count={}\n",
            self.enabled,
            self.capacity,
            self.records.len(),
            occurrences
        );
        for record in &self.records {
            let severity = match record.severity {
                Severity::Warning => "warning",
                Severity::Error => "error",
            };
            output.push_str(&format!(
                "code={} severity={} fingerprint(non-cryptographic-default-hasher)={} count={} message={}\n",
                record.code, severity, record.fingerprint, record.count, record.message
            ));
        }
        output
    }
}

fn fingerprint(message: &str) -> String {
    let mut hasher = DefaultHasher::new();
    message.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn bound(message: String) -> String {
    if message.len() <= MAX_MESSAGE_BYTES {
        return message;
    }
    let end = message
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= MAX_MESSAGE_BYTES)
        .last()
        .unwrap_or(0);
    message[..end].to_owned()
}

fn redact(raw: &str) -> String {
    const VALUES: [&str; 7] = [
        "token=",
        "api_key=",
        "password=",
        "authorization:",
        "prompt=",
        "audio=",
        "chat=",
    ];
    let bytes = raw.as_bytes();
    let mut output = String::with_capacity(raw.len().min(MAX_MESSAGE_BYTES));
    let mut index = 0;
    while index < bytes.len() {
        if let Some(marker) = VALUES
            .iter()
            .find(|marker| matches_at(bytes, index, marker))
        {
            output.push_str(&raw[index..index + marker.len()]);
            output.push_str("[REDACTED]");
            index += marker.len();
            while index < bytes.len() && !value_end(bytes[index], marker == &"authorization:") {
                index += 1;
            }
        } else if matches_at(bytes, index, "/home/") {
            output.push_str("/home/[REDACTED]");
            index += "/home/".len();
            while index < bytes.len() && bytes[index] != b'/' && !bytes[index].is_ascii_whitespace()
            {
                index += 1;
            }
        } else if matches_at(bytes, index, "c:\\users\\") {
            output.push_str("C:\\Users\\[REDACTED]");
            index += "c:\\users\\".len();
            while index < bytes.len()
                && bytes[index] != b'\\'
                && bytes[index] != b'/'
                && !bytes[index].is_ascii_whitespace()
            {
                index += 1;
            }
        } else {
            let character = raw[index..].chars().next().expect("valid UTF-8");
            output.push(character);
            index += character.len_utf8();
        }
    }
    output
}

fn matches_at(bytes: &[u8], index: usize, marker: &str) -> bool {
    bytes
        .get(index..index + marker.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(marker.as_bytes()))
}

fn value_end(byte: u8, authorization: bool) -> bool {
    if authorization {
        matches!(byte, b'\r' | b'\n')
    } else {
        byte.is_ascii_whitespace() || matches!(byte, b',' | b';' | b'&')
    }
}
