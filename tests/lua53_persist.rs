//! The persist prototype layout, driven by a host decoder for constants and nested prototypes.

use lua_resume::lua53::chunk::{Function, read_chunk};
use lua_resume::lua53::persist::{ValueDecoder, read_body, read_prototype};
use lua_resume::lua53::{Constant, Cursor, ReadError};

const SAMPLE: &[u8] = include_bytes!("fixtures/lua53/sample.luac");

fn compact(family: u8, value: i64) -> Vec<u8> {
    if let Ok(byte) = u8::try_from(value) {
        return vec![family | 8, byte];
    }
    let mut out = vec![family | 0xc];
    out.extend_from_slice(&i32::try_from(value).unwrap().to_le_bytes());
    out
}

fn count(value: usize) -> Vec<u8> {
    compact(0x10, i64::try_from(value).unwrap())
}

fn raw_string(text: &[u8]) -> Vec<u8> {
    let mut out = count(text.len());
    out.extend_from_slice(text);
    out
}

fn constant(value: &Constant) -> Vec<u8> {
    match value {
        Constant::Nil => vec![1],
        Constant::Bool(true) => vec![2],
        Constant::Bool(false) => vec![3],
        Constant::Int(n) => compact(0x10, *n),
        Constant::Str(text) => {
            let mut out = compact(0x20, i64::try_from(text.len()).unwrap());
            out.extend_from_slice(text);
            out
        }
        Constant::Float(_) => unreachable!("the fixture has no floats"),
    }
}

/// Encodes a chunk function in the persist layout; nested functions are inline.
fn encode(function: &Function) -> Vec<u8> {
    let p = &function.0;
    let mut out = vec![0xa0, p.num_params, u8::from(p.is_vararg), p.max_stack];
    out.extend_from_slice(&u16::try_from(p.line_defined).unwrap().to_le_bytes());
    out.extend_from_slice(&u16::try_from(p.last_line_defined).unwrap().to_le_bytes());
    out.extend(count(p.code.len()));
    p.code
        .iter()
        .for_each(|w| out.extend_from_slice(&w.to_le_bytes()));
    out.extend(count(p.constants.len()));
    p.constants.iter().for_each(|k| out.extend(constant(k)));
    out.extend(count(p.children.len()));
    p.children
        .iter()
        .for_each(|child| out.extend(encode(child)));
    out.extend(count(p.upvalues.len()));
    for up in &p.upvalues {
        out.extend_from_slice(&[u8::from(up.in_stack), up.index]);
        out.extend(raw_string(&up.name));
    }
    out.extend(raw_string(&p.source));
    out.extend(count(p.line_info.len()));
    p.line_info
        .iter()
        .for_each(|l| out.extend_from_slice(&u16::try_from(*l).unwrap().to_le_bytes()));
    out.extend(count(p.locals.len()));
    for local in &p.locals {
        out.extend(raw_string(&local.name));
        out.extend_from_slice(&u16::try_from(local.start_pc).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(local.end_pc).unwrap().to_le_bytes());
    }
    out
}

/// A host decoder with the small value grammar the encoder above writes.
struct Host;

impl ValueDecoder for Host {
    type Constant = Constant;
    type Child = Function;

    fn constant(&mut self, cur: &mut Cursor<'_>) -> Result<Constant, ReadError> {
        let tag = cur.clone().u8()?;
        Ok(match tag & 0xf0 {
            0 => {
                cur.u8()?;
                match tag {
                    1 => Constant::Nil,
                    2 => Constant::Bool(true),
                    3 => Constant::Bool(false),
                    _ => return Err(cur.error("unknown constant")),
                }
            }
            0x10 => Constant::Int(cur.compact(0x10)?),
            0x20 => {
                let length = cur.compact(0x20)?;
                let length =
                    cur.bounded(u64::try_from(length).map_err(|_| cur.error("negative"))?, 1)?;
                Constant::Str(cur.take(length)?.to_vec())
            }
            _ => return Err(cur.error("unknown constant")),
        })
    }

    fn child(&mut self, cur: &mut Cursor<'_>) -> Result<Function, ReadError> {
        read_body(cur, self).map(Function)
    }
}

#[test]
fn every_function_of_a_chunk_survives_the_persist_layout() {
    let chunk = read_chunk(SAMPLE).unwrap();
    for function in chunk.main.flatten() {
        let bytes = encode(function);
        let (decoded, end) = read_prototype(&bytes, 0, &mut Host).unwrap();
        assert_eq!(end, bytes.len());
        assert_eq!(decoded, function.0);
    }
}

#[test]
fn decoding_starts_at_the_given_position() {
    let chunk = read_chunk(SAMPLE).unwrap();
    let mut bytes = vec![0xee; 7];
    bytes.extend(encode(&chunk.main));
    bytes.push(0x99);
    let (decoded, end) = read_prototype(&bytes, 7, &mut Host).unwrap();
    assert_eq!(decoded, chunk.main.0);
    assert_eq!(end, bytes.len() - 1);
}

#[test]
fn every_truncation_is_an_error() {
    let bytes = encode(&read_chunk(SAMPLE).unwrap().main);
    for length in 0..bytes.len() {
        assert!(
            read_prototype(&bytes[..length], 0, &mut Host).is_err(),
            "length {length}"
        );
    }
}

#[test]
fn changed_bytes_never_panic() {
    let bytes = encode(&read_chunk(SAMPLE).unwrap().main);
    for index in 0..bytes.len() {
        for value in [0u8, 0x1c, 0xff] {
            let mut copy = bytes.clone();
            copy[index] = value;
            let _ = read_prototype(&copy, 0, &mut Host);
        }
    }
}

#[test]
fn a_count_larger_than_the_data_is_refused() {
    let mut bytes = vec![0xa0, 0, 0, 2, 1, 0, 2, 0];
    bytes.extend(compact(0x10, 1_000_000));
    let error = read_prototype(&bytes, 0, &mut Host).unwrap_err();
    assert!(error.message.contains("count exceeds"), "{error}");
}

#[test]
fn another_tag_is_not_a_prototype() {
    let error = read_prototype(&[0x80, 0, 0], 0, &mut Host).unwrap_err();
    assert!(error.message.contains("tag"), "{error}");
}

#[test]
fn back_references_are_the_hosts_to_resolve() {
    struct Shared(Vec<u8>);
    impl ValueDecoder for Shared {
        type Constant = Constant;
        type Child = u8;
        fn constant(&mut self, cur: &mut Cursor<'_>) -> Result<Constant, ReadError> {
            Host.constant(cur)
        }
        fn child(&mut self, cur: &mut Cursor<'_>) -> Result<u8, ReadError> {
            let reference = cur.compact(0x30)?;
            self.0
                .push(u8::try_from(reference).map_err(|_| cur.error("range"))?);
            Ok(u8::try_from(reference).unwrap())
        }
    }
    let mut bytes = vec![0xa0, 0, 0, 2, 1, 0, 2, 0];
    bytes.extend(count(0));
    bytes.extend(count(0));
    bytes.extend(count(2));
    bytes.extend(compact(0x30, 5));
    bytes.extend(compact(0x30, 5));
    bytes.extend(count(0));
    bytes.extend(raw_string(b"@x"));
    bytes.extend(count(0));
    bytes.extend(count(0));
    let mut host = Shared(Vec::new());
    let (decoded, end) = read_prototype(&bytes, 0, &mut host).unwrap();
    assert_eq!(decoded.children, [5, 5]);
    assert_eq!(end, bytes.len());
    assert_eq!(host.0, [5, 5]);
}
