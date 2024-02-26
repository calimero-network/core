use libp2p::identity::{Keypair, PublicKey};

pub fn start_network(app_id: String, keypair: Keypair) {
    //start new network

    //store provided keypair or generate new one

    //set user as owner of network

    //start frontend
}

pub fn join_network(app_id: String, keypair: Keypair) {
    //find network by app id

    //store keypair

    //join existing network

    //sync state

    //generate auth token
}

pub fn generate_challenge() -> String {
    //create challenge for login on another machine
    "challenge".to_string()
}

pub fn login(challenge: String, signature: Vec<u8>) {
    //challenge signature based on keypair already on node

    //retrieve peer id based on public key

    //generate auth token and peer id
}

//middleware - tbd
//we can authenticate each request by checking msg signature for simplicity
