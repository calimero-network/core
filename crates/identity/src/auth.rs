use libp2p::identity::Keypair;

pub fn get_peer_keypair() -> Result<Keypair, String> {
    let private_key = "...";

    let private_key = bs58::decode(private_key)
        .into_vec()
        .map_err(|_| "Invalid PrivKey base 58".to_string())?;

    let keypair = Keypair::from_protobuf_encoding(&private_key)
        .map_err(|_| "Decoding PrivKey failed.".to_string())?;
    Ok(keypair)
}

pub fn verify_peer_auth(msg: &[u8], signature: &[u8]) -> Result<bool, String> {
    let keypair = get_peer_keypair()?;

    // let signed = keypair.sign(msg).unwrap();
    // let encoded = bs58::encode(signed).into_string();
    // println!("{:?}", encoded);
    // println!("blabla {:?}", bs58::encode("blabla").into_string());

    Ok(keypair.public().verify(msg, signature))
}
