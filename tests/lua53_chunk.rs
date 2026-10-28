//! Reading Lua 5.3 chunks: both headers, the debug tables, and malformed input.

use lua_resume::lua53::Constant;
use lua_resume::lua53::chunk::{Header, read_chunk};

const SAMPLE: &[u8] = include_bytes!("fixtures/lua53/sample.luac");

/// The same chunk with the three-byte `MapData` size header.
fn map_data(standard: &[u8]) -> Vec<u8> {
    let mut out = standard[..12].to_vec();
    out.extend_from_slice(&[4, 4, 8]);
    out.extend_from_slice(&standard[17..]);
    out
}

#[test]
fn standard_chunk_reads_every_function() {
    let chunk = read_chunk(SAMPLE).unwrap();
    assert_eq!(chunk.header, Header::Standard);
    let all = chunk.main.flatten();
    let spans: Vec<(u32, u32)> = all
        .iter()
        .map(|f| (f.0.line_defined, f.0.last_line_defined))
        .collect();
    assert_eq!(spans, [(0, 0), (2, 5), (7, 23), (16, 20)]);
    assert!(all.iter().all(|f| f.0.source == b"@Lua/sample.lua"));
}

#[test]
fn run_function_carries_its_debug_information() {
    let chunk = read_chunk(SAMPLE).unwrap();
    let run = &chunk.main.flatten()[2].0;
    assert_eq!(run.num_params, 2);
    assert!(run.is_vararg);
    assert_eq!(run.line_info.len(), run.code.len());
    let names: Vec<&[u8]> = run.locals.iter().map(|l| l.name.as_slice()).collect();
    assert!(names.contains(&b"total".as_slice()) && names.contains(&b"(for generator)".as_slice()));
    assert_eq!(
        run.upvalues
            .iter()
            .map(|u| u.name.as_slice())
            .collect::<Vec<_>>(),
        [b"note".as_slice(), b"_ENV".as_slice()]
    );
    assert!(run.constants.contains(&Constant::Str(b"tostring".to_vec())));
}

#[test]
fn map_data_header_reads_the_same_functions() {
    let standard = read_chunk(SAMPLE).unwrap();
    let variant = read_chunk(&map_data(SAMPLE)).unwrap();
    assert_eq!(variant.header, Header::MapData);
    assert_eq!(variant.main, standard.main);
}

#[test]
fn every_truncation_is_an_error() {
    for length in 0..SAMPLE.len() {
        assert!(read_chunk(&SAMPLE[..length]).is_err(), "length {length}");
    }
}

#[test]
fn changed_bytes_never_panic() {
    for index in 0..SAMPLE.len() {
        for value in [0u8, 0x7f, 0xff] {
            let mut bytes = SAMPLE.to_vec();
            bytes[index] = value;
            let _ = read_chunk(&bytes);
        }
    }
}

#[test]
fn huge_counts_are_refused_before_allocation() {
    let mut bytes = SAMPLE.to_vec();
    // Header (34 bytes), the 16-byte source name, two line numbers and three byte fields.
    let code_count = 34 + 16 + 8 + 3;
    bytes[code_count..code_count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let error = read_chunk(&bytes).unwrap_err();
    assert!(error.message.contains("count exceeds"), "{error}");
}

#[test]
fn foreign_headers_are_named() {
    let mut bytes = SAMPLE.to_vec();
    bytes[4] = 0x54;
    assert!(read_chunk(&bytes).unwrap_err().message.contains("5.3"));
    assert!(
        read_chunk(b"local x = 1")
            .unwrap_err()
            .message
            .contains("precompiled")
    );
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = SAMPLE.to_vec();
    bytes.push(0);
    assert!(read_chunk(&bytes).unwrap_err().message.contains("after"));
}
