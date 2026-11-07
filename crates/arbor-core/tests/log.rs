mod common;

use std::io::{BufWriter, Write};

use arbor_core::{EventLog, ReadError, read_events};
use common::full_session_events;

#[test]
fn appended_events_read_back_one_per_line() {
    let events = full_session_events();
    let mut log = EventLog::new(Vec::new());
    for event in &events {
        log.append(event).unwrap();
    }
    let bytes = log.get_ref();
    assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), events.len());
    assert_eq!(read_events(&bytes[..]).unwrap(), events);
}

#[test]
fn each_append_is_flushed_before_it_returns() {
    let mut log = EventLog::new(BufWriter::with_capacity(1 << 16, Vec::new()));
    log.append(&full_session_events()[0]).unwrap();
    let written = log.get_ref().get_ref();
    assert!(
        written.ends_with(b"\n"),
        "nothing reached the underlying writer"
    );
}

#[test]
fn a_torn_final_line_from_a_crash_is_ignored() {
    let events = full_session_events();
    let mut log = EventLog::new(Vec::new());
    log.append(&events[0]).unwrap();
    log.append(&events[1]).unwrap();
    let mut bytes = log.get_ref().clone();
    bytes.write_all(br#"{"type":"checkpoint_rej"#).unwrap();
    assert_eq!(read_events(&bytes[..]).unwrap(), events[..2]);
}

#[test]
fn a_corrupt_line_before_the_end_is_an_error() {
    let events = full_session_events();
    let mut bytes = Vec::new();
    let mut log = EventLog::new(&mut bytes);
    log.append(&events[0]).unwrap();
    bytes.extend_from_slice(b"not json\n");
    let mut log = EventLog::new(&mut bytes);
    log.append(&events[1]).unwrap();
    assert!(matches!(
        read_events(&bytes[..]),
        Err(ReadError::Corrupt { line: 2, .. })
    ));
}
