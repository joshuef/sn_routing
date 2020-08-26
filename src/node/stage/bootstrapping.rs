// Copyright 2020 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use crate::{
    comm::Comm,
    error::Result,
    id::{FullId, P2pNode},
    messages::{BootstrapResponse, Message, Variant, VerifyStatus},
    relocation::{RelocatePayload, SignedRelocateDetails},
    rng::MainRng,
    section::EldersInfo,
    time::Duration,
};
use quic_p2p::{IncomingConnections, IncomingMessages, Message as QuicP2pMsg};

use bytes::Bytes;
use fxhash::FxHashSet;
use std::{iter, net::SocketAddr};
use xor_name::Prefix;

use log::warn;

/// Time after which bootstrap is cancelled (and possibly retried).
pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(20);

// The bootstrapping stage - node is trying to find the section to join.
pub(crate) struct Bootstrapping {
    // Using `FxHashSet` for deterministic iteration order.
    pending_requests: FxHashSet<SocketAddr>,
    relocate_details: Option<SignedRelocateDetails>,
    full_id: FullId,
    rng: MainRng,
    comm: Comm,
}

impl Bootstrapping {
    pub async fn bootstrap(
        relocate_details: Option<SignedRelocateDetails>,
        full_id: FullId,
        rng: MainRng,
        mut comm: Comm,
    ) -> Result<()> {

        // Why do we need this in stages?

        let mut bootstrap = Self {
            pending_requests: Default::default(),
            relocate_details,
            full_id,
            rng,
            comm,
        };

        let mut incoming = bootstrap.comm.listen_events()?;

        while let Some( mut incoming_msgs) = incoming.next().await {
            trace!(
                "New connection established by peer {}",
                incoming_msgs.remote_addr()
            );

            while let Some(msg) = incoming_msgs.next().await {
                match msg {
                    QuicP2pMsg::UniStream { bytes, src } => {
                        trace!(
                            "New message ({} bytes) received on a uni-stream from: {}",
                            bytes.len(),
                            src
                        );
                        // Since it's arriving on a uni-stream we treat it as a Node
                        // message which we need to be processed by us, as well as
                        // reported to the event stream consumer.
                        // handle_node_message(bytes, src).await



                        match Message::from_bytes(&bytes) {
                            Err(error) => {
                                debug!("Failed to deserialize message: {:?}", error);
                                // None
                            }
                            Ok(msg) => {
                                trace!("try handle message in bootstrap process{:?}", msg);
                                let _ = bootstrap.process_message(src, msg);
                                let _ = bootstrap.handle_bootstrap_response(src, msg);
                                // let event_to_relay = if let Variant::BootstrapResponse(res) = msg.variant() {
                                //     // Some(Event::MessageReceived {
                                //     //     content: bytes.clone(),
                                //     //     src: msg.src().src_location(),
                                //     //     dst: *msg.dst(),
                                //     // })
                                // } else {
                                //     // None
                                //     // Do nothing
                                // };
                    
                                // Some((event_to_relay, Some((sender, msg))))
                            }
                        }
                    },
                    _ => {
                        // warn!("Non node-msg received during routing bootstrap")
                    }
                }
            }
            // Self::spawn_messages_handler(events_tx.clone(), incoming_msgs, xorname)
        }


        Ok(())
    }

    pub async fn process_message(&mut self, sender: SocketAddr, msg: Message) -> Result<()> {
        match msg.variant() {
            Variant::BootstrapResponse(_) => {
                verify_message(&msg)?;
                Ok(())
            }

            Variant::NeighbourInfo { .. }
            | Variant::UserMessage(_)
            | Variant::BouncedUntrustedMessage(_)
            | Variant::DKGMessage { .. }
            | Variant::DKGOldElders { .. } => {
                debug!("Unknown message from {}: {:?} ", sender, msg);
                // self.msg_backlog.push(msg.into_queued(Some(sender)))
                Ok(())
            }

            Variant::NodeApproval(_)
            | Variant::EldersUpdate { .. }
            | Variant::Promote { .. }
            | Variant::NotifyLagging { .. }
            | Variant::Relocate(_)
            | Variant::MessageSignature(_)
            | Variant::BootstrapRequest(_)
            | Variant::JoinRequest(_)
            | Variant::ParsecRequest(..)
            | Variant::ParsecResponse(..)
            | Variant::Ping
            | Variant::BouncedUnknownMessage { .. }
            | Variant::Vote { .. } => {
                debug!("Useless message from {}: {:?}", sender, msg);
                Ok(())
            }
        }
    }

    pub fn comm(&mut self) -> &mut Comm {
        &mut self.comm
    }

    pub async fn send_message_to_target(
        &mut self,
        recipient: &SocketAddr,
        msg: Bytes,
    ) -> Result<()> {
        self.comm.send_message_to_target(recipient, msg).await
    }

    pub async fn handle_bootstrap_response(
        &mut self,
        sender: P2pNode,
        response: BootstrapResponse,
    ) -> Result<Option<JoinParams>> {
        // Ignore messages from peers we didn't send `BootstrapRequest` to.
        if !self.pending_requests.contains(sender.peer_addr()) {
            debug!(
                "Ignoring BootstrapResponse from unexpected peer: {}",
                sender,
            );
            //TODO?? core.transport.disconnect(*sender.peer_addr());
            return Ok(None);
        }

        match response {
            BootstrapResponse::Join {
                elders_info,
                section_key,
            } => {
                info!(
                    "Joining a section {:?} (given by {:?})",
                    elders_info, sender
                );

                let relocate_payload = self.join_section(&elders_info)?;
                Ok(Some(JoinParams {
                    elders_info,
                    section_key,
                    relocate_payload,
                }))
            }
            BootstrapResponse::Rebootstrap(new_conn_infos) => {
                info!(
                    "Bootstrapping redirected to another set of peers: {:?}",
                    new_conn_infos
                );
                self.reconnect_to_new_section(new_conn_infos).await?;
                Ok(None)
            }
        }
    }

    pub async fn send_bootstrap_request(&mut self, dst: SocketAddr) -> Result<()> {
        //let token = core.timer.schedule(BOOTSTRAP_TIMEOUT);
        //let _ = self.timeout_tokens.insert(token, dst);

        let xorname = match &self.relocate_details {
            Some(details) => *details.destination(),
            None => *self.full_id.public_id().name(),
        };

        debug!("Sending BootstrapRequest to {}.", dst);
        self.comm
            .send_direct_message(&self.full_id, &dst, Variant::BootstrapRequest(xorname))
            .await
    }

    async fn reconnect_to_new_section(&mut self, new_conn_infos: Vec<SocketAddr>) -> Result<()> {
        // TODO???
        /*for addr in self.pending_requests.drain() {
            core.transport.disconnect(addr);
        }*/

        for conn_info in new_conn_infos {
            self.send_bootstrap_request(conn_info).await?;
        }

        Ok(())
    }

    fn join_section(&mut self, elders_info: &EldersInfo) -> Result<Option<RelocatePayload>> {
        let relocate_details = self.relocate_details.take();
        let destination = match &relocate_details {
            Some(details) => *details.destination(),
            None => *self.full_id.public_id().name(),
        };
        let old_full_id = self.full_id.clone();

        // Use a name that will match the destination even after multiple splits
        let extra_split_count = 3;
        let name_prefix = Prefix::new(
            elders_info.prefix.bit_count() + extra_split_count,
            destination,
        );

        if !name_prefix.matches(self.full_id.public_id().name()) {
            let new_full_id = FullId::within_range(&mut self.rng, &name_prefix.range_inclusive());
            info!("Changing name to {}.", new_full_id.public_id().name());
            self.full_id = new_full_id;
        }

        if let Some(details) = relocate_details {
            let payload = RelocatePayload::new(details, self.full_id.public_id(), &old_full_id)?;
            Ok(Some(payload))
        } else {
            Ok(None)
        }
    }
}

pub(crate) struct JoinParams {
    pub elders_info: EldersInfo,
    pub section_key: bls::PublicKey,
    pub relocate_payload: Option<RelocatePayload>,
}

fn verify_message(msg: &Message) -> Result<()> {
    msg.verify(iter::empty())
        .and_then(VerifyStatus::require_full)
}
