use super::*;

#[test]
fn test_verify_signature_valid() {
	let challenge = "89qdrkz1egXlJ2wwF1tcZpuFT0LXT4AHhnAnFvG3N/E=";
	let message = "helloworld";
	let app = "me";
	let curl = "http://127.0.0.1:2428/admin/confirm-wallet";
	let signature_base64 = "rkBQLYN7xxe1oetSfktrqL5jgVsZWKNvKZJmoZLNh756KIUBseYIzK3Dt17O60aPMl6S17lDnIlLVLOLdi5OCw==";
	let public_key = "ed25519:DxdDEdfg4sARk2YteEvp6KsqUGAgKyCZkYTqrboGWwiV";
	
	assert!(verify_near_signature(
		challenge,
		message,
		app,
		curl,
		signature_base64,
		public_key
	));
}

#[test]
fn test_verify_signature_invalid() {
	let challenge = "89qdrkz1egXlJ2wwF1tcZpuFT0LXT4AHhnAnFvG3N/E=";
	let message = "helloworld";
	let app = "me";
	let curl = "http://127.0.0.1:2428/admin/confirm-wallet";
	let signature_base64 = "rkBQLYN7xxe1oetSfktrqL5jgVsZWKNvKZJmoZLNh756KIUBseYIzK3Dt17O60aPMl6S17lDnIlLVsOLdi5OCw==";
	let public_key = "ed25519:DxdDEdfg4sARk2YteEvp6KsqUGAgKyCZkYTqrboGWwiV";
	
	assert!(!verify_near_signature(
		challenge,
		message,
		app,
		curl,
		signature_base64,
		public_key
	));
}
