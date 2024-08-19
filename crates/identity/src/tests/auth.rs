use super::*;

#[test]
fn test_recover() {
	let account = "0x04a4e95eeebe44a0f37b75f40957445d2d16a88c".to_string();
	let message = "blabla";
	let message_hash = eth_message(message);
	let signature ="0x15a88c2b8f3f3a0549c88dcbdba063de517ce55e68fdf2ad443f2c24f71904c740f212f0d74b94e271b9d549a8825894d0869b173709a5e798ec6997a1c72d5b1b".trim_start_matches("0x");
	let signature = hex::decode(signature).unwrap();
	println!("{} {:?} {:?}", account, message_hash, signature);
	let recovery_id = signature[64] as i32 - 27;
	let pubkey = recover(&message_hash, &signature[..64], recovery_id);
	assert!(pubkey.is_ok());
	let pubkey = pubkey.unwrap();
	let pubkey = format!("{:02X?}", pubkey);
	assert_eq!(account, pubkey)
}
