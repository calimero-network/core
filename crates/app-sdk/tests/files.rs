//! `files` against the in-memory host.

use std::io::{Read, Write};

use calimero_app_sdk::files::{self, FileId};
use sha2::{Digest, Sha256};

#[test]
fn a_file_round_trips_through_writer_and_reader() {
    let mut w = files::create().unwrap();
    w.write_all(b"hello ").unwrap();
    w.write_all(b"world").unwrap();
    w.flush().unwrap();
    let id = w.finish().unwrap();

    let mut text = String::new();
    files::open(&id).unwrap().read_to_string(&mut text).unwrap();
    assert_eq!(text, "hello world");

    let info = files::info(&id).unwrap();
    assert_eq!(info.size, 11);
    assert_eq!(info.sha256, Sha256::digest(b"hello world").to_vec());
}

#[test]
fn reads_come_in_the_size_of_the_buffer_and_end_with_zero() {
    let mut w = files::create().unwrap();
    w.write_all(b"abcde").unwrap();
    let id = w.finish().unwrap();
    let mut r = files::open(&id).unwrap();
    let mut buf = [0u8; 3];
    assert_eq!(r.read(&mut buf).unwrap(), 3);
    assert_eq!(&buf, b"abc");
    assert_eq!(r.read(&mut buf).unwrap(), 2);
    assert_eq!(&buf[..2], b"de");
    assert_eq!(r.read(&mut buf).unwrap(), 0);
}

#[test]
fn an_unfinished_writer_leaves_no_file() {
    let mut w = files::create().unwrap();
    w.write_all(b"lost").unwrap();
    drop(w);
    let empty = files::create().unwrap().finish().unwrap();
    assert_eq!(files::info(&empty).unwrap().size, 0);
    let lost = FileId::from(hex::encode(Sha256::digest(b"lost")));
    assert!(files::info(&lost).is_none());
}

#[test]
fn unknown_files_are_errors_and_delete_removes() {
    let ghost = FileId::from("nope".to_owned());
    assert!(files::info(&ghost).is_none());
    assert_eq!(files::open(&ghost).err().unwrap().code, "Files");
    assert_eq!(files::delete(&ghost).unwrap_err().code, "Files");

    let mut w = files::create().unwrap();
    w.write_all(b"x").unwrap();
    let id = w.finish().unwrap();
    files::delete(&id).unwrap();
    assert!(files::info(&id).is_none());
}

#[test]
fn a_file_id_is_a_plain_json_string() {
    let id = FileId::from("abc".to_owned());
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"abc\"");
    assert_eq!(serde_json::from_str::<FileId>("\"abc\"").unwrap(), id);
    assert_eq!(id.to_string(), "abc");
}
