use std::io::Cursor;

use spirit::{SignalTransport, schema::signal::Input};
use triad_runtime::{FrameBody, LengthPrefixedCodec};

#[test]
fn transport_rejects_length_prefixed_raw_nota_text() {
    let nota =
        b"(Record (([(Technology Intelligence)] Decision [text must not be daemon wire] Maximum Minimum Zero []) ([text must not be daemon wire] None)))";
    let bytes = LengthPrefixedCodec::default()
        .encode_body(&FrameBody::new(nota.to_vec()))
        .expect("length-prefixed frame");
    let mut transport = SignalTransport::new(Cursor::new(bytes));

    assert!(
        transport.read_input().is_err(),
        "daemon wire transport must reject length-prefixed raw NOTA bytes"
    );
}

#[test]
fn transport_rejects_length_prefixed_garbage_bytes() {
    let bytes = LengthPrefixedCodec::default()
        .encode_body(&FrameBody::new([0_u8; 16]))
        .expect("length-prefixed frame");
    let mut transport = SignalTransport::new(Cursor::new(bytes));

    assert!(
        transport.read_input().is_err(),
        "daemon wire transport must reject arbitrary bytes"
    );
}

#[test]
fn generated_input_decoder_rejects_raw_nota_text_directly() {
    let nota =
        b"(Record (([(Technology Intelligence)] Decision [text must not be signal frame] Maximum Minimum Zero []) ([text must not be signal frame] None)))";

    assert!(
        Input::decode_signal_frame(nota).is_err(),
        "schema-emitted binary decoder must reject raw NOTA text"
    );
}
