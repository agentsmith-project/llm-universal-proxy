use bytes::Bytes;

pub(crate) trait StreamObservationTransform: Send {
    fn transform_chunk(&mut self, bytes: &Bytes) -> Vec<Bytes>;

    fn finish(&mut self) -> Vec<Bytes> {
        Vec::new()
    }
}
