use std::io::{Cursor, Write};

use spirit_next::{Input, SignalTransport};

struct LengthPrefixedFrame<'payload> {
    payload: &'payload [u8],
}

impl<'payload> LengthPrefixedFrame<'payload> {
    fn new(payload: &'payload [u8]) -> Self {
        Self { payload }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let length = u32::try_from(self.payload.len()).expect("test payload fits u32");
        let mut bytes = Vec::with_capacity(4 + self.payload.len());
        bytes
            .write_all(&length.to_be_bytes())
            .expect("write length prefix");
        bytes.write_all(self.payload).expect("write payload");
        bytes
    }
}

#[test]
fn transport_rejects_length_prefixed_raw_nota_text() {
    let nota = b"(Record ([[socket-negative]] Decision [text must not be daemon wire] Maximum))";
    let bytes = LengthPrefixedFrame::new(nota).to_bytes();
    let mut transport = SignalTransport::new(Cursor::new(bytes));

    assert!(
        transport.read_input().is_err(),
        "daemon wire transport must reject length-prefixed raw NOTA bytes"
    );
}

#[test]
fn transport_rejects_length_prefixed_garbage_bytes() {
    let bytes = LengthPrefixedFrame::new(&[0_u8; 16]).to_bytes();
    let mut transport = SignalTransport::new(Cursor::new(bytes));

    assert!(
        transport.read_input().is_err(),
        "daemon wire transport must reject arbitrary bytes"
    );
}

#[test]
fn generated_input_decoder_rejects_raw_nota_text_directly() {
    let nota = b"(Record ([[socket-negative]] Decision [text must not be signal frame] Maximum))";

    assert!(
        Input::decode_signal_frame(nota).is_err(),
        "schema-emitted binary decoder must reject raw NOTA text"
    );
}
