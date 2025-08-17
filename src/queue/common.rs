use std::time::Duration;
use thiserror::Error;

pub type Bytes = Vec<u8>;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum TrySendError {
    #[error("channel is full")]
    ChannelFullError,
    #[error("channel is disconnected")]
    ChannelDisconnected,
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum RecvTimeoutError {
    #[error("recv timeout")]
    Timeout,
    #[error("channel is disconnected")]
    ChannelDisconnected,
}

pub trait QueueChannelSender {
    fn try_send(&self, data: Bytes) -> Result<(), TrySendError>;
}

pub trait QueueChannelReceiver {
    fn recv_timeout(&self, timeout: Duration) -> Result<Bytes, RecvTimeoutError>;
}
