use crossbeam::channel::{unbounded, Receiver, Sender};
use eyre::Result;
use pnet::datalink::{self, Channel, ChannelType, Config};
use std::time::Duration;

use super::common::{Bytes, QueueChannelReceiver, QueueChannelSender, RecvTimeoutError, TrySendError};

pub struct QdiscSender {
    sender: Sender<Bytes>,
}

impl QueueChannelSender for QdiscSender {
    fn try_send(&self, data: Bytes) -> Result<(), TrySendError> {
        if let Err(e) = self.sender.try_send(data) {
            match e {
                crossbeam::channel::TrySendError::Full(_) => return Err(TrySendError::ChannelFullError),
                crossbeam::channel::TrySendError::Disconnected(_) => return Err(TrySendError::ChannelDisconnected),
            }
        }
        Ok(())
    }
}

pub struct QdiscReceiver {
    receiver: Receiver<Bytes>,
}

impl QueueChannelReceiver for QdiscReceiver {
    fn recv_timeout(&self, timeout: Duration) -> Result<Bytes, RecvTimeoutError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(data) => Ok(data),
            Err(e) => match e {
                crossbeam::channel::RecvTimeoutError::Timeout => Err(RecvTimeoutError::Timeout),
                crossbeam::channel::RecvTimeoutError::Disconnected => Err(RecvTimeoutError::ChannelDisconnected),
            },
        }
    }
}

pub struct QdiscHandle {}

pub fn qdisc_channel(interface_name: &str) -> Result<(QdiscSender, QdiscReceiver, QdiscHandle)> {
    let peer_name = format!("{}_peer", interface_name);
    let (tx_to_veth, rx_from_veth) = unbounded::<Bytes>();
    let (tx_from_peer, rx_from_peer) = unbounded::<Bytes>();

    let veth_interface = interface_name.to_string();
    let veth_interface_clone = veth_interface.clone();
    let _peer_interface = peer_name.clone();

    std::thread::spawn(move || {
        let config = Config {
            write_buffer_size: 4096,
            read_buffer_size: 4096,
            read_timeout: None,
            write_timeout: None,
            channel_type: ChannelType::Layer2,
            bpf_fd_attempts: 1000,
            linux_fanout: None,
            promiscuous: true,
            socket_fd: None,
        };

        let interfaces = datalink::interfaces();
        let veth_iface = interfaces
            .into_iter()
            .find(|iface| iface.name == veth_interface)
            .expect("Could not find veth interface");

        let (mut tx_veth, _) = match datalink::channel(&veth_iface, config) {
            Ok(Channel::Ethernet(tx, _)) => (tx, ()),
            _ => panic!("Failed to create channel to veth interface"),
        };

        while let Ok(packet) = rx_from_veth.recv() {
            let _ = tx_veth.send_to(&packet, None);
        }
    });

    std::thread::spawn(move || {
        let config = Config {
            write_buffer_size: 4096,
            read_buffer_size: 4096,
            read_timeout: None,
            write_timeout: None,
            channel_type: ChannelType::Layer2,
            bpf_fd_attempts: 1000,
            linux_fanout: None,
            promiscuous: true,
            socket_fd: None,
        };

        let interfaces = datalink::interfaces();
        let veth_iface = interfaces
            .into_iter()
            .find(|iface| iface.name == veth_interface_clone)
            .expect("Could not find veth interface");

        let (_, mut rx_veth) = match datalink::channel(&veth_iface, config) {
            Ok(Channel::Ethernet(_, rx)) => ((), rx),
            _ => panic!("Failed to create channel to veth interface"),
        };

        while let Ok(packet) = rx_veth.next() {
            let _ = tx_from_peer.send(packet.to_vec());
        }
    });

    let qdisc_sender = QdiscSender { sender: tx_to_veth };

    let qdisc_receiver = QdiscReceiver { receiver: rx_from_peer };

    let qdisc_handle = QdiscHandle {};

    Ok((qdisc_sender, qdisc_receiver, qdisc_handle))
}
