use std::str::from_utf8;

use calimero_store::Store;
use local_ip_address::local_ip;
use rcgen::{CertificateParams, DnType};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{parse_x509_pem, ParsedExtension};

use crate::admin::storage::ssl::{get_ssl, insert_or_update_ssl, SSLCert};

pub async fn get_certificate(store: Store) -> eyre::Result<(Vec<u8>, Vec<u8>)> {
    let certificate = match get_ssl(store.clone())? {
        Some(cert) => check_certificate(store.clone(), cert).await?,
        None => generate_certificate(store.clone()).await?,
    };
    write_out_instructions();
    Ok(certificate)
}

async fn generate_certificate(store: Store) -> eyre::Result<(Vec<u8>, Vec<u8>)> {
    // Get the local IP address
    let local_ip = local_ip()?;

    // Generate a self-signed certificate for IP address and localhost
    let subject_alt_names = vec![
        local_ip.to_string(),
        "127.0.0.1".to_string(),
        "localhost".to_string(),
    ];

    // Certificate name
    let certificate_name = format!(
        "Calimero self-signed certificate for local IP: {:?}",
        &local_ip.to_string()
    );

    // Create certificate parameters
    let mut params = CertificateParams::new(subject_alt_names.clone())?;
    // Set the distinguished name
    params
        .distinguished_name
        .push(DnType::CommonName, certificate_name);

    let key_pair = match get_ssl(store.clone())? {
        Some(ssl) => {
            let key = from_utf8(ssl.key())?;
            rcgen::KeyPair::from_pem(key)?
        }
        None => rcgen::KeyPair::generate()?,
    };

    // Generate the certificate with the customized parameters
    let cert = CertificateParams::self_signed(params, &key_pair)?;
    // Serialize the certificate and private key to PEM format
    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();

    drop(insert_or_update_ssl(store.clone(), &cert_pem, &key_pem)?);

    Ok((cert_pem, key_pem))
}

async fn check_certificate(store: Store, cert: SSLCert) -> eyre::Result<(Vec<u8>, Vec<u8>)> {
    let (cert_pem, key_pem) = (cert.cert(), cert.key());
    let (_, pem) = parse_x509_pem(cert_pem)?;
    let x509_cert = pem.parse_x509()?;

    // Get the local IP address
    let local_ip = local_ip()?;

    // Flag for found IP address in certificate
    let mut ip_found = false;

    for ext in x509_cert.extensions() {
        // Parse the extension
        let parsed_ext = ext.parsed_extension();

        // Check if it's a SubjectAlternativeName extension
        if let ParsedExtension::SubjectAlternativeName(san) = parsed_ext {
            // Iterate over the general names
            for gn in &san.general_names {
                // Check if the general name is an IP address
                if let GeneralName::IPAddress(dns_name) = gn {
                    ip_found = match local_ip {
                        std::net::IpAddr::V4(ip) => ip.octets() == *dns_name,
                        std::net::IpAddr::V6(ip) => ip.octets() == *dns_name,
                    };
                    if ip_found {
                        break;
                    }
                }
            }
        }
    }

    if !ip_found {
        return generate_certificate(store.clone()).await;
    }
    Ok((cert_pem.clone(), key_pem.clone()))
}

fn write_out_instructions() {
    println!("*******************************************************************************");
    println!("To install the generated self-signed SSL certificate, follow the steps in our documentation:");
    println!("https://calimero-network.github.io/getting-started/setup");
    println!("*******************************************************************************");
}
