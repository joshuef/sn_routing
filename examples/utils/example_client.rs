// Copyright 2015 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under (1) the MaidSafe.net Commercial License,
// version 1.0 or later, or (2) The General Public License (GPL), version 3, depending on which
// licence you accepted on initial access to the Software (the "Licences").
//
// By contributing code to the SAFE Network Software, or to this project generally, you agree to be
// bound by the terms of the MaidSafe Contributor Agreement, version 1.0.  This, along with the
// Licenses can be found in the root directory of this project at LICENSE, COPYING and CONTRIBUTOR.
//
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.
//
// Please review the Licences for the specific language governing permissions and limitations
// relating to use of the SAFE Network Software.

extern crate log;
extern crate time;
extern crate routing;
extern crate xor_name;
extern crate sodiumoxide;
extern crate maidsafe_utilities;

use std::sync::mpsc;
use std::thread;
use self::sodiumoxide::crypto;
use self::xor_name::XorName;
use self::routing::{FullId, Event, Data, DataRequest, Authority, ResponseContent, ResponseMessage, Client};

/// Network Client.
#[allow(unused)]
pub struct ExampleClient {
    routing_client: Client,
    receiver: mpsc::Receiver<Event>,
    full_id: FullId,
}

#[allow(unused)]
impl ExampleClient {
    /// Client constructor.
    pub fn new() -> ExampleClient {
        let (sender, receiver) = mpsc::channel::<Event>();
        let sign_keys = crypto::sign::gen_keypair();
        let encrypt_keys = crypto::box_::gen_keypair();
        let full_id = FullId::with_keys(encrypt_keys.clone(), sign_keys.clone());
        let routing_client = Client::new(sender, Some(full_id)).unwrap();

        // Wait for Connected event from Routing
        loop {
            if let Ok(event) = receiver.try_recv() {
                if let Event::Connected = event {
                    println!("Client Connected to network");
                    break;
                }
            }

            thread::sleep(::std::time::Duration::from_secs(1));
        }

        ExampleClient {
            routing_client: routing_client,
            receiver: receiver,
            full_id: FullId::with_keys(encrypt_keys, sign_keys),
        }
    }

    /// Get from network.
    pub fn get(&mut self, request: DataRequest) -> Option<Data> {
        unwrap_result!(self.routing_client
                           .send_get_request(Authority::NaeManager(request.name()),
                                             request.clone()));

        // Wait for Get success event from Routing
        loop {
            if let Ok(event) = self.receiver.try_recv() {
                match event {
                    Event::Response(ResponseMessage{ content: ResponseContent::GetSuccess(data, _), .. }) => {
                        return Some(data)
                    }
                    Event::Response(ResponseMessage{ content: ResponseContent::GetFailure{ external_error_indicator, .. }, .. }) => {
                        error!("Failed to Get {:?}: {:?}", request.name(), unwrap_result!(String::from_utf8(external_error_indicator)));
                        return None
                    }
                    _ => ()
                }
            }

            thread::sleep(::std::time::Duration::from_secs(1));
        }
    }

    /// Put to network.
    pub fn put(&self, data: Data) {
        let data_name = data.name();
        unwrap_result!(self.routing_client
                           .send_put_request(Authority::ClientManager(*self.name()), data));
        // Wait for Put success event from Routing
        loop {
            if let Ok(event) = self.receiver.try_recv() {
                if let Event::Response(ResponseMessage{ content: ResponseContent::PutSuccess(..), .. }) = event {
                    println!("Successfully stored {:?}", data_name);
                    break;
                }
            }

            thread::sleep(::std::time::Duration::from_secs(1));
        }
    }

    /// Post data onto the network.
    #[allow(unused)]
    pub fn post(&self) {
        unimplemented!()
    }

    /// Delete data from the network.
    #[allow(unused)]
    pub fn delete(&self) {
        unimplemented!()
    }

    /// Return network name.
    pub fn name(&self) -> &XorName {
        self.full_id.public_id().name()
    }
}
