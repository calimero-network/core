use tokio::fs;
use std::path::{Path, PathBuf};

use local_ip_address::local_ip;
use rcgen::{CertificateParams, DnType};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{parse_x509_pem, ParsedExtension};

pub async fn get_certificate() -> eyre::Result<(Vec<u8>, Vec<u8>)> {
    // Get the path to the certificate and key files
    let certificate_dir = get_certificate_dir();
    let cert_path = certificate_dir.join("cert.pem");
    let key_path = certificate_dir.join("key.pem");

    // Ensure paths exist
    if !cert_path.exists() || !key_path.exists() {
        let (cert_pem, key_pem) = generate_certificate().await?;
        write_out_instructions();
        return Ok((cert_pem, key_pem));
    }

    // Check if a certificate contains local IP address
    let (cert_pem, key_pem) = check_certificate().await?;
    write_out_instructions();
    Ok((cert_pem, key_pem))
}

async fn generate_certificate() -> eyre::Result<(Vec<u8>, Vec<u8>)> {
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

    // Load keypair if it exists or generate a new one
    let certificate_dir = get_certificate_dir();
    let key_file_path = certificate_dir.join("key.pem");
    let key_pair = if key_file_path.exists() {
        let key_pem = fs::read_to_string(key_file_path).await?;
        rcgen::KeyPair::from_pem(&key_pem)?
    } else {
        rcgen::KeyPair::generate().unwrap()
    };

    // Generate the certificate with the customized parameters
    let cert = CertificateParams::self_signed(params, &key_pair).unwrap();

    // Serialize the certificate and private key to PEM format
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    // Get the path to the certificate and key files
    let cert_file_path = certificate_dir.join("cert.pem");
    let key_file_path = certificate_dir.join("key.pem");

    // Save the certificate and key to files
    fs::write(cert_file_path, cert_pem.clone()).await?;
    fs::write(key_file_path, key_pem.clone()).await?;

    Ok((cert_pem.as_bytes().to_vec(), key_pem.as_bytes().to_vec()))
}

async fn delete_certificate() -> eyre::Result<()> {
    let certificate_dir = get_certificate_dir();
    let cert_path = certificate_dir.join("cert.pem");
    fs::remove_file(cert_path).await?;

    Ok(())
}

async fn check_certificate() -> eyre::Result<(Vec<u8>, Vec<u8>)> {
    let certificate_dir = get_certificate_dir();
    let cert_file = certificate_dir.join("cert.pem");
    let key_file = certificate_dir.join("key.pem");
    // Read the certificate file
    let cert_pem = fs::read_to_string(cert_file.clone()).await?;
    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())?;
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
        delete_certificate().await?;
        return Ok(generate_certificate().await?);
    }

    let cert_pem = fs::read(&cert_file).await?;
    let key_pem = fs::read(&key_file).await?;
    Ok((cert_pem, key_pem))
}

fn write_out_instructions() {
    println!("*******************************************************************************");
    println!("To install the generated self-signed SSL certificate, follow the steps in our documentation:");
    println!("https://https://calimero-network.github.io/getting-started/setup");
    println!("*******************************************************************************");
}

pub fn get_certificate_dir() -> PathBuf {
    // Convert the current file path to a PathBuf
    let current_file_path = Path::new(file!());
    // Get the directory containing the current file
    let parent_dir = current_file_path
        .parent()
        .expect("Failed to get parent directory");
    // Get the grandparent directory
    let server_dir = parent_dir.parent().expect("Failed to get parent directory");
    // Get certificates directory
    let certificate_dir = server_dir.join("certificates");

    certificate_dir
}
