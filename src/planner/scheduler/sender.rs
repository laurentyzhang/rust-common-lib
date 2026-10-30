pub type Sender = [u8; 20];

/// A sender's nonce increment to apply before starting a sequence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SenderNonceOffset {
    pub sender: Sender,
    pub offset: u64,
}
