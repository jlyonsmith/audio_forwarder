use bytes::BytesMut;
use prost::Message;
use std::io;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec}; // Assuming you are using the 'protobuf' crate

// Define a generic ProstCodec
pub struct ProstCodec<T>(LengthDelimitedCodec, std::marker::PhantomData<T>);

impl<T> ProstCodec<T> {
    pub fn new() -> Self {
        // Default configuration for length delimited framing
        ProstCodec(LengthDelimitedCodec::new(), std::marker::PhantomData)
    }
}

// Implement Decoder for your Prost message type
impl<T: Message + Default> Decoder for ProstCodec<T> {
    type Item = T;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Use the inner LengthDelimitedCodec to get a framed payload (BytesMut)
        match self.0.decode(src)? {
            Some(frame) => {
                // Decode the protobuf message from the frame payload
                T::decode(frame)
                    .map(Some)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            None => Ok(None),
        }
    }
}

// Implement Encoder for your Prost message type
impl<T: Message> Encoder<T> for ProstCodec<T> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // Encode the protobuf message to a Vec<u8> first
        let mut buf = Vec::with_capacity(item.encoded_len());
        item.encode(&mut buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        // Use the inner LengthDelimitedCodec to frame the buffer
        self.0.encode(buf.into(), dst)
    }
}
