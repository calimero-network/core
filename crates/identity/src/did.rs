use std::fs;

use eyre::WrapErr;
use serde::{Serialize, Deserialize};
use calimero_primitives::utils::{FileManager, FileOperations};

const DID_FILE: &str = "did.json";

#[derive(Serialize, Deserialize, Debug)]
pub struct VerificationMethod {
    id: String,
    type_: String,
    controller: String,
    #[serde(rename = "publicKeyBase58")]
    public_key: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Service {
    id: String,
    type_: String,
    #[serde(rename = "serviceEndpoint")]
    service_endpoint: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DidDocument {
    #[serde(rename = "@context")]
    context: String,
    id: String,
    #[serde(rename = "verificationMethod")]
    verification_method: Vec<VerificationMethod>,
    authentication: Vec<String>,
    service: Vec<Service>,
}

impl DidDocument {
    pub fn new(id: String, public_key: String) -> Self {
        // Initialize the document with the provided id and public key
        DidDocument {
            context: "https://www.w3.org/ns/did/v1".to_string(),
            id: id.clone(),
            verification_method: vec![VerificationMethod {
                id: format!("{}#keys-1", id),
                type_: "Ed25519VerificationKey2018".to_string(),
                controller: id.clone(),
                public_key,
            }],
            authentication: vec![format!("{}#keys-1", id)],
            service: vec![], // Initialize without any services
        }
    }

    // Function to add a service to the DID document
    pub fn add_service(&mut self, service: Service) {
        self.service.push(service);
    }

    // Serialize the DID document to a JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self)
    }

    // Deserialize a JSON string to a DID document
    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    pub fn exists(dir: &camino::Utf8Path) -> bool {
        dir.join(DID_FILE).is_file()
    }

    pub fn load(dir: &camino::Utf8Path) -> eyre::Result<Self> {
        let path = dir.join(DID_FILE);
        let content = fs::read_to_string(&path).wrap_err_with(|| {
            format!(
                "failed to read configuration from {:?}",
                dir.join(DID_FILE)
            )
        })?;

        DidDocument::from_json(&content).map_err(Into::into)
    }

    pub fn save(&self, dir: &camino::Utf8Path) -> eyre::Result<()> {
        let content = self.to_json().wrap_err_with(|| {
            format!(
                "failed to serialize configuration to {:?}",
                dir.join(DID_FILE)
            )
        })?;

        fs::write(&path, content).wrap_err_with(|| {
            format!(
                "failed to write configuration to {:?}",
                dir.join(DID_FILE)
            )
        })?;
        Ok(())
    }
}

struct Did {
    did_document: DidDocument,
    file_manager: FileManager,
}

impl Did {
    pub fn new(did_document: DidDocument, file_manager: FileManager) -> Self {
        Did {
            did_document,
            file_manager,
        }
    }

    pub fn exists(&self) -> bool {
        self.file_manager.contains_file(DID_FILE)
    }

    pub fn load(&self) -> eyre::Result<DidDocument> {
        self.did_document.load(&self.file_manager.debug_location())
    }

    pub fn save(&self) -> eyre::Result<()> {
        self.did_document.save(&self.file_manager.debug_location())
    }
}


#[cfg(test)]
mod tests {
    use crate::did;

    use super::*;

    #[test]
    fn test_did_document_creation() {
        let did_doc = DidDocument::new("did:calimero:123".to_string(), "1234567890".to_string());
        println!("{:?}", did_doc.to_json().unwrap());
        did_doc.save(&Utf8PathBuf::new()).unwrap();
    }

    #[test]
    fn test_add_service() {
        let mut did_doc = DidDocument::new("did:calimero:123".to_string(), "1234567890".to_string());
        let service = Service {
            id: "did:example:123#node-keys".to_string(),
            type_: "NodeKeysService".to_string(),
            service_endpoint: "https://example.com/node_keys".to_string(),
        };
        did_doc.add_service(service);
        println!("{:?}", did_doc.to_json().unwrap());
    }


    #[test]
    fn test_load_did() {
        let did_doc = DidDocument::load(&Utf8PathBuf::new()).unwrap();
        println!("{:?}", did_doc.to_json().unwrap());
    }
}
